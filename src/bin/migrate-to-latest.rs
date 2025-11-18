use async_trait::async_trait;
use libsql::{Builder, Connection, Transaction};
use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::io::Read;
use thiserror::Error;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let adapter = create_adapter_from_env().await?;
    run_migrations(&adapter).await?;
    Ok(())
}

#[derive(Error, Debug)]
pub enum MigrationError {
    #[error("Adapter error: {0}")]
    Adapter(#[from] AdapterError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Checksum mismatch for migration {0}. Expected {1}, found {2}")]
    ChecksumMismatch(String, String, String),
    #[error("Failed to read migration file {0}")]
    ReadFile(String),
}

#[derive(Debug, Clone)]
pub struct AppliedMigration {
    pub name: String,
    pub checksum: String,
}

pub async fn run_migrations<B>(backend: &B) -> Result<(), MigrationError>
where
    B: MigrationBackend + ?Sized,
{
    const BOOTSTRAP_MIGRATIONS_SQL: &str = r#"
        CREATE TABLE IF NOT EXISTS __migrations (
            name TEXT PRIMARY KEY,
            checksum TEXT NOT NULL,
            description TEXT,
            executed_by TEXT,
            executed_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );
    "#;

    // Ensure the control table exists even when there are no migration files on disk.
    backend
        .ensure_migrations_table(BOOTSTRAP_MIGRATIONS_SQL)
        .await?;

    // 2. Get all applied migrations from the database
    let applied_migrations = backend.fetch_applied_migrations().await?;

    // 3. Get all migration files from the filesystem
    let mut migration_files: Vec<_> = fs::read_dir("migrations")?
        .map(|res| res.map(|e| e.path()))
        .collect::<Result<Vec<_>, std::io::Error>>()?
        .into_iter()
        .filter(|path| path.is_file() && path.extension().map_or(false, |ext| ext == "sql"))
        .collect();
    migration_files.sort();

    // 4. Verify checksums of applied migrations
    for (i, applied) in applied_migrations.iter().enumerate() {
        if i >= migration_files.len() {
            // This should not happen if migrations are only added
            break;
        }
        let file_path = &migration_files[i];
        let file_name = file_path.file_name().unwrap().to_str().unwrap().to_string();

        if file_name != applied.name {
            // This indicates a problem, e.g., a file was renamed
            continue;
        }

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

    // 5. Apply pending migrations
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
pub trait MigrationBackend: Send + Sync {
    async fn ensure_migrations_table(&self, bootstrap_sql: &str) -> Result<(), AdapterError>;
    async fn fetch_applied_migrations(&self) -> Result<Vec<AppliedMigration>, AdapterError>;
    async fn apply_migration(
        &self,
        name: &str,
        sql: &str,
        checksum: &str,
    ) -> Result<(), AdapterError>;
}

#[derive(Clone)]
pub struct LibSqlAdapter {
    conn: Connection,
}

impl LibSqlAdapter {
    pub fn new(conn: Connection) -> Self {
        Self { conn }
    }

    fn conn(&self) -> &Connection {
        &self.conn
    }
}

#[async_trait]
impl MigrationBackend for LibSqlAdapter {
    async fn ensure_migrations_table(&self, bootstrap_sql: &str) -> Result<(), AdapterError> {
        self.conn().execute_batch(bootstrap_sql).await?;
        Ok(())
    }

    async fn fetch_applied_migrations(&self) -> Result<Vec<AppliedMigration>, AdapterError> {
        let mut rows = self
            .conn()
            .query(
                "SELECT name, checksum FROM __migrations ORDER BY name ASC",
                libsql::params![],
            )
            .await?;

        let mut applied = Vec::new();
        while let Some(row) = rows.next().await? {
            applied.push(AppliedMigration {
                name: row.get(0)?,
                checksum: row.get(1)?,
            });
        }

        Ok(applied)
    }

    async fn apply_migration(
        &self,
        name: &str,
        sql: &str,
        checksum: &str,
    ) -> Result<(), AdapterError> {
        let tx = self.conn().transaction().await?;
        apply_migration_in_transaction(tx, name, sql, checksum).await
    }
}

async fn apply_migration_in_transaction(
    tx: Transaction,
    name: &str,
    sql: &str,
    checksum: &str,
) -> Result<(), AdapterError> {
    tx.execute_batch(sql).await?;
    tx.execute(
        "INSERT INTO __migrations (name, checksum, description, executed_by) VALUES (?1, ?2, ?3, ?4)",
        libsql::params![name, checksum, "Initial schema", "system"],
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

async fn create_adapter_from_env() -> anyhow::Result<LibSqlAdapter> {
    let db_path = env::var("LIBSQL_DB_PATH").unwrap_or_else(|_| "migrations.db".to_string());
    let database = Builder::new_local(db_path).build().await?;
    let conn = database.connect()?;
    Ok(LibSqlAdapter::new(conn))
}

#[derive(Debug)]
pub struct AdapterError(Box<dyn std::error::Error + Send + Sync>);

impl AdapterError {
    pub fn new<E>(err: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self(Box::new(err))
    }
}

impl From<libsql::Error> for AdapterError {
    fn from(err: libsql::Error) -> Self {
        AdapterError::new(err)
    }
}

impl std::fmt::Display for AdapterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for AdapterError {}
