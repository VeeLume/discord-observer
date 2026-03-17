use anyhow::Result;
use sqlx::migrate::MigrateDatabase;
use sqlx::{Pool, Sqlite, sqlite::SqlitePoolOptions};

#[derive(Clone)]
pub struct Db {
    pub pool: Pool<Sqlite>,
}

impl Db {
    pub async fn connect(db_url: &str) -> Result<Self> {
        if !Sqlite::database_exists(db_url).await.unwrap_or(false) {
            Sqlite::create_database(db_url).await?;
        }
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(db_url)
            .await?;

        // run migrations in your /migrations folder
        sqlx::migrate!().run(&pool).await?;
        Ok(Self { pool })
    }
}
