//! Biblioteca de migrações compartilhada pelas aplicações deste projeto.
//!
//! A ideia aqui é separar regras de negócio (como listar arquivos `.sql`,
//! validar checksums e registrar o histórico) da implementação de acesso ao
//! banco. Isso facilita testar e reaproveitar o mesmo fluxo com diferentes
//! bancos.

// Importamos `async_trait` porque traits no Rust não aceitam métodos async
// nativamente. Esse macro “embrulha” o trait para que possamos declarar as
// funções como `async` e usar `.await` dentro das implementações.
use async_trait::async_trait;
// `sha2` nos fornece o algoritmo SHA-256 e os tipos necessários para gerar
// checksums. Usamos isso para garantir que o conteúdo aplicado corresponde ao
// que está salvo na tabela de controle do banco.
use sha2::{Digest, Sha256};
// `std::fs` e `std::io::Read` são usados para percorrer a pasta de migrações e
// ler os bytes de cada arquivo `.sql` do disco.
use std::fs;
use std::io::Read;
// `thiserror` reduz a verbosidade na criação de enums de erro que implementam
// `std::error::Error`, permitindo mensagens mais amigáveis.
use thiserror::Error;

#[derive(Error, Debug)]
/// Enum básico com todos os erros que podem acontecer durante uma migração.
/// Cada variante descreve a natureza do problema para facilitar o debug.
pub enum MigrationError {
    /// Problemas vindos do adaptador (banco de dados). Como o adaptador pode
    /// ser qualquer implementação (libsql, Postgres, etc), convertemos cada
    /// erro concreto para `AdapterError` e depois para esta variante.
    #[error("Adapter error: {0}")]
    Adapter(#[from] AdapterError),
    /// Falhas em operações básicas de arquivo (abrir, listar, ler bytes, …).
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Disparado quando o checksum calculado em disco não bate com o que já
    /// foi gravado no banco. Essa validação evita aplicar scripts alterados.
    #[error("Checksum mismatch for migration {0}. Expected {1}, found {2}")]
    ChecksumMismatch(String, String, String),
    /// Indicamos quando não conseguimos converter o arquivo para `String`
    /// (por exemplo, caracteres inválidos em UTF-8).
    #[error("Failed to read migration file {0}")]
    ReadFile(String),
}

#[derive(Debug, Clone)]
/// Representa uma linha da tabela `__migrations` no banco. Guardamos o nome
/// do arquivo executado e o checksum correspondente.
pub struct AppliedMigration {
    pub name: String,
    pub checksum: String,
}

/// Função principal que orquestra a execução das migrações. Ela recebe um
/// `backend` genérico que implementa [`MigrationBackend`]. Dessa forma,
/// podemos reutilizar o mesmo fluxo com qualquer banco ou tecnologia,
/// contanto que exista um adaptador compatível.
pub async fn run_migrations<B>(backend: &B) -> Result<(), MigrationError>
where
    // `MigrationBackend + ?Sized` permite aceitar tanto tipos concretos quanto
    // referências trait. O bound `Send + Sync` está definido no trait para que
    // os adaptadores possam ser compartilhados em contextos async sem violar
    // regras de concorrência (Send = pode ser movido entre threads; Sync =
    // referências para o tipo podem ser compartilhadas por múltiplas threads).
    B: MigrationBackend + ?Sized,
{
    // Este SQL garante que a tabela de controle exista. Mesmo se não houver
    // arquivos, precisamos da tabela para registrar futuras execuções.
    const BOOTSTRAP_MIGRATIONS_SQL: &str = r#"
        CREATE TABLE IF NOT EXISTS __migrations (
            name TEXT PRIMARY KEY,
            checksum TEXT NOT NULL,
            description TEXT,
            executed_by TEXT,
            executed_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );
    "#;

    // 1. Cria a tabela `__migrations` caso não exista. O adaptador decide
    // como executar o SQL (transação, conexão, etc.).
    backend
        .ensure_migrations_table(BOOTSTRAP_MIGRATIONS_SQL)
        .await?;

    // 2. Busca a lista de migrações já aplicadas para determinar até onde o
    // banco está atualizado.
    let applied_migrations = backend.fetch_applied_migrations().await?;

    // 3. Varre a pasta `migrations/`, pega somente arquivos `.sql`, ordena
    // alfabeticamente (garantindo que 0001_... execute antes de 0002_...).
    let mut migration_files: Vec<_> = fs::read_dir("migrations")?
        .map(|res| res.map(|e| e.path()))
        .collect::<Result<Vec<_>, std::io::Error>>()?
        .into_iter()
        .filter(|path| path.is_file() && path.extension().map_or(false, |ext| ext == "sql"))
        .collect();
    migration_files.sort();

    // 4. Valida os checksums de tudo que já foi aplicado. Isso protege contra
    // o cenário "alguém editou um arquivo já aplicado".
    for (i, applied) in applied_migrations.iter().enumerate() {
        if i >= migration_files.len() {
            break;
        }
        let file_path = &migration_files[i];
        let file_name = file_path.file_name().unwrap().to_str().unwrap().to_string();

        if file_name != applied.name {
            continue;
        }

        // Lemos o arquivo inteiro para gerar o hash e comparar com o valor no
        // banco.
        let mut file = fs::File::open(file_path)?;
        let mut content = Vec::new();
        file.read_to_end(&mut content)?;
        let checksum = format!("{:x}", Sha256::digest(&content));

        if checksum != applied.checksum {
            return Err(MigrationError::ChecksumMismatch(
                file_name,
                applied.checksum.clone(),
                checksum,
            ));
        }
    }

    // 5. Executa os arquivos restantes (aqueles que não foram validados no
    // passo anterior). `skip(applied_migrations.len())` garante que aplicamos
    // apenas o que está faltando.
    for file_path in migration_files.iter().skip(applied_migrations.len()) {
        let file_name = file_path.file_name().unwrap().to_str().unwrap().to_string();

        let mut file = fs::File::open(file_path)?;
        let mut content = Vec::new();
        file.read_to_end(&mut content)?;
        let checksum = format!("{:x}", Sha256::digest(&content));

        let sql =
            String::from_utf8(content).map_err(|_| MigrationError::ReadFile(file_name.clone()))?;

        backend
            .apply_migration(&file_name, sql.as_str(), &checksum)
            .await?;

        println!("Applied migration: {}", file_name);
    }

    Ok(())
}

#[async_trait]
/// Trait que precisa ser implementado por qualquer adaptador de banco. As
/// funções retornam `AdapterError` para encapsular erros específicos do
/// driver. O `Send + Sync` citado anteriormente garante que o objeto pode ser
/// usado em contextos multithread dentro de `tokio`.
pub trait MigrationBackend: Send + Sync {
    /// Executa o SQL de bootstrap para criar a tabela `__migrations`.
    async fn ensure_migrations_table(&self, bootstrap_sql: &str) -> Result<(), AdapterError>;
    /// Busca e retorna em ordem (geralmente alfabética) as migrações já
    /// registradas.
    async fn fetch_applied_migrations(&self) -> Result<Vec<AppliedMigration>, AdapterError>;
    /// Aplica uma nova migração e registra o checksum correspondente.
    async fn apply_migration(
        &self,
        name: &str,
        sql: &str,
        checksum: &str,
    ) -> Result<(), AdapterError>;
}

#[derive(Debug)]
/// Estrutura simples que embrulha qualquer `std::error::Error`. Mantemos o
/// erro dentro de um `Box<dyn Error + Send + Sync>` para que diferentes
/// adaptadores possam converter seus erros específicos sem perda de
/// informação. O `Send + Sync` assegura que o erro pode atravessar threads.
pub struct AdapterError(Box<dyn std::error::Error + Send + Sync>);

impl AdapterError {
    /// Cria um `AdapterError` a partir de qualquer erro que implemente os
    /// traits mínimos (`Error + Send + Sync + 'static`).
    pub fn new<E>(err: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self(Box::new(err))
    }
}

impl std::fmt::Display for AdapterError {
    /// Apenas delegamos a formatação para o erro original armazenado na caixa.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for AdapterError {}
