//! Binário que usa a biblioteca de migrações para atualizar um banco libSQL local.
//!
//! A responsabilidade aqui é conectar na base, implementar o trait
//! [`MigrationBackend`] usando libSQL como driver e delegar o restante para a
//! biblioteca compartilhada.

// `async_trait` novamente permite declarar funções async dentro do trait que
// implementaremos (MigrationBackend).
use async_trait::async_trait;
// Tipos principais do libSQL usados: `Builder` cria/conecta no banco, `Connection`
// executa comandos e `Transaction` garante atomicidade na aplicação das migrações.
use libsql::{Builder, Connection, Transaction};
// Reexportamos da nossa biblioteca as peças necessárias: função que orquestra
// as migrações, trait que precisamos implementar e tipos auxiliares.
use rust_test::migrate_to_latest::{
    AdapterError, AppliedMigration, MigrationBackend, run_migrations,
};
use std::env;

#[tokio::main]
/// Função principal. Ela apenas cria o adaptador com base nas variáveis de
/// ambiente e delega a execução das migrações para a biblioteca.
async fn main() -> anyhow::Result<()> {
    // `create_adapter_from_env` lê `LIBSQL_DB_PATH` (ou usa `migrations.db` como
    // padrão), abre uma conexão libSQL e já retorna o adaptador pronto.
    let adapter = create_adapter_from_env().await?;
    // A biblioteca cuida do fluxo completo (listar arquivos, gerar checksum,
    // chamar o backend). Aqui só precisamos passar uma referência ao adaptador.
    run_migrations(&adapter).await?;
    Ok(())
}

#[derive(Clone)]
/// Adaptador concreto que implementa `MigrationBackend` usando a API do libSQL.
/// Como armazenamos somente a `Connection`, conseguimos clonar o adaptador sem
/// abrir novas conexões.
pub struct LibSqlAdapter {
    conn: Connection,
}

impl LibSqlAdapter {
    /// Construtor simples. Recebe a conexão já aberta e guarda internamente.
    pub fn new(conn: Connection) -> Self {
        Self { conn }
    }

    /// Método auxiliar para acessar a conexão. Mesmo sendo privado, ajuda a
    /// centralizar qualquer mudança futura (por exemplo, adicionar métricas).
    fn conn(&self) -> &Connection {
        &self.conn
    }
}

#[async_trait]
impl MigrationBackend for LibSqlAdapter {
    /// Cria a tabela de controle rodando o SQL fornecido. `map_err` converte o
    /// `libsql::Error` em `AdapterError` usando o construtor genérico definido na
    /// biblioteca.
    async fn ensure_migrations_table(&self, bootstrap_sql: &str) -> Result<(), AdapterError> {
        self.conn()
            .execute_batch(bootstrap_sql)
            .await
            .map_err(AdapterError::new)?;
        Ok(())
    }

    /// Busca as migrações já aplicadas no banco. Retornamos um `Vec` para que a
    /// biblioteca possa comparar com os arquivos em disco.
    async fn fetch_applied_migrations(&self) -> Result<Vec<AppliedMigration>, AdapterError> {
        let mut rows = self
            .conn()
            .query(
                "SELECT name, checksum FROM __migrations ORDER BY name ASC",
                libsql::params![],
            )
            .await
            .map_err(AdapterError::new)?;

        let mut applied = Vec::new();
        // Iteramos linha a linha da consulta async. Cada chamada de `row.get`
        // pode falhar (coluna inexistente, tipo inválido, etc.), então também
        // convertemos esses erros para `AdapterError`.
        while let Some(row) = rows.next().await.map_err(AdapterError::new)? {
            applied.push(AppliedMigration {
                name: row.get(0).map_err(AdapterError::new)?,
                checksum: row.get(1).map_err(AdapterError::new)?,
            });
        }

        Ok(applied)
    }

    /// Recebe o conteúdo de uma nova migração e a aplica dentro de uma
    /// transação. Separar essa lógica facilita testar ou trocar o driver no
    /// futuro.
    async fn apply_migration(
        &self,
        name: &str,
        sql: &str,
        checksum: &str,
    ) -> Result<(), AdapterError> {
        // `transaction()` abre uma transação explícita para que a execução do SQL e o
        // registro na tabela `__migrations` sejam atômicos: ou tudo acontece ou nada
        // acontece. Assim evitamos inconsistências em caso de erro.
        let tx = self.conn().transaction().await.map_err(AdapterError::new)?;
        apply_migration_in_transaction(tx, name, sql, checksum).await
    }
}

/// Executa efetivamente a migração dentro de uma transação já aberta. Essa
/// função fica fora da implementação do trait para deixar o código mais
/// reaproveitável/tutorial.
async fn apply_migration_in_transaction(
    tx: Transaction,
    name: &str,
    sql: &str,
    checksum: &str,
) -> Result<(), AdapterError> {
    // Primeiro rodamos o script SQL do arquivo de migração.
    tx.execute_batch(sql).await.map_err(AdapterError::new)?;
    // Depois registramos o arquivo no quadro de controle para evitar aplicar a
    // mesma migração novamente.
    tx.execute(
        "INSERT INTO __migrations (name, checksum, description, executed_by) VALUES (?1, ?2, ?3, ?4)",
        libsql::params![name, checksum, "Initial schema", "system"],
    )
    .await
    .map_err(AdapterError::new)?;
    // Por fim, persistimos a transação. Se algum passo tiver falhado, o erro
    // anterior teria abortado a função antes desta linha.
    tx.commit().await.map_err(AdapterError::new)?;
    Ok(())
}

/// Lê variáveis de ambiente necessárias e constrói o `LibSqlAdapter`.
async fn create_adapter_from_env() -> anyhow::Result<LibSqlAdapter> {
    // Permite customizar o caminho do arquivo `.db`. Caso a variável não exista,
    // usamos `migrations.db` como padrão para facilitar ambientes locais.
    let db_path = env::var("LIBSQL_DB_PATH").unwrap_or_else(|_| "migrations.db".to_string());
    // `Builder::new_local` abre um banco libSQL baseado em arquivo. Poderíamos
    // trocar por outros builders caso queira apontar para um servidor remoto.
    let database = Builder::new_local(db_path).build().await?;
    // `connect` devolve a conexão (`Connection`), que é tudo o que o adaptador
    // precisa para cumprir o contrato do trait.
    let conn = database.connect()?;
    Ok(LibSqlAdapter::new(conn))
}
