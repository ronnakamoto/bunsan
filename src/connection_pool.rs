use deadpool::managed::{Manager, Metrics, Object, Pool, RecycleResult};
use log::info;
use reqwest::Client;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::time::{interval, Duration};

pub struct ClientManager {
    client: Client,
}

impl ClientManager {
    pub fn new(client: Client) -> Self {
        Self { client }
    }
}

impl Manager for ClientManager {
    type Type = Client;
    type Error = reqwest::Error;

    async fn create(&self) -> Result<Client, Self::Error> {
        Ok(self.client.clone())
    }

    async fn recycle(
        &self,
        _client: &mut Client,
        _metrics: &Metrics,
    ) -> RecycleResult<Self::Error> {
        // For HTTP clients, we don't need to do anything special for recycling.
        // If you want to add any checks or resets, you can do so here.
        Ok(())
    }
}

pub type ClientPool = Pool<ClientManager>;
pub type PooledClient = Object<ClientManager>;

#[derive(Clone)]
pub struct DynamicClientPool {
    pool: ClientPool,
    min_size: usize,
    max_size: usize,
    current_size: Arc<AtomicUsize>,
}

impl DynamicClientPool {
    pub fn new(min_size: usize, max_size: usize) -> Self {
        let client = Client::builder()
            .pool_max_idle_per_host(100) // Adjust this value based on your needs
            .build()
            .expect("Failed to create HTTP client");
        let manager = ClientManager::new(client);
        let pool = Pool::builder(manager)
            .max_size(max_size)
            .build()
            .expect("Failed to create connection pool");

        let current_size = Arc::new(AtomicUsize::new(min_size));

        let pool_clone = pool.clone();
        let current_size_clone = current_size.clone();

        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(60)); // Adjust every minute
            loop {
                interval.tick().await;
                Self::adjust_pool_size(&pool_clone, &current_size_clone, min_size, max_size).await;
            }
        });

        Self {
            pool,
            min_size,
            max_size,
            current_size,
        }
    }

    pub async fn get(&self) -> Result<PooledClient, deadpool::managed::PoolError<reqwest::Error>> {
        self.pool.get().await
    }

    pub fn status(&self) -> deadpool::managed::Status {
        self.pool.status()
    }

    async fn adjust_pool_size(
        pool: &ClientPool,
        current_size: &AtomicUsize,
        min_size: usize,
        max_size: usize,
    ) {
        let status = pool.status();
        let current = current_size.load(Ordering::Relaxed);
        let new_size = if status.available == 0 && current < max_size {
            // All connections are in use, increase pool size
            (current + (current / 2)).min(max_size)
        } else if status.available > current / 2 && current > min_size {
            // More than half of connections are idle, decrease pool size
            (current - (current / 4)).max(min_size)
        } else {
            current
        };

        if new_size != current {
            pool.resize(new_size);
            current_size.store(new_size, Ordering::Relaxed);
            info!("Adjusted pool size from {} to {}", current, new_size);
        }
    }

    pub fn clone(&self) -> Self {
        Self {
            pool: self.pool.clone(),
            min_size: self.min_size,
            max_size: self.max_size,
            current_size: self.current_size.clone(),
        }
    }
}

pub fn create_dynamic_pool(min_size: usize, max_size: usize) -> DynamicClientPool {
    DynamicClientPool::new(min_size, max_size)
}
