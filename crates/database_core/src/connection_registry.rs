use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, RwLock};

use crate::connection::{ConnectionConfig, DatabaseConnection};
use crate::schema::DatabaseSchema;

pub struct RegisteredConnection {
    pub config: ConnectionConfig,
    pub connection: Arc<dyn DatabaseConnection>,
    pub schema: Option<DatabaseSchema>,
}

static CONNECTION_REGISTRY: LazyLock<RwLock<HashMap<String, RegisteredConnection>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

pub fn register_connection(
    name: String,
    config: ConnectionConfig,
    connection: Arc<dyn DatabaseConnection>,
    schema: Option<DatabaseSchema>,
) {
    match CONNECTION_REGISTRY.write() {
        Ok(mut registry) => {
            registry.insert(
                name,
                RegisteredConnection {
                    config,
                    connection,
                    schema,
                },
            );
        }
        Err(error) => {
            log::error!("Failed to acquire connection registry write lock: {error}");
        }
    }
}

pub fn unregister_connection(name: &str) {
    match CONNECTION_REGISTRY.write() {
        Ok(mut registry) => {
            registry.remove(name);
        }
        Err(error) => {
            log::error!("Failed to acquire connection registry write lock: {error}");
        }
    }
}

pub fn get_connection(
    name: &str,
) -> Option<(Arc<dyn DatabaseConnection>, ConnectionConfig, Option<DatabaseSchema>)> {
    let registry = match CONNECTION_REGISTRY.read() {
        Ok(guard) => guard,
        Err(error) => {
            log::error!("Failed to acquire connection registry read lock: {error}");
            return None;
        }
    };
    let entry = registry.get(name)?;
    Some((
        entry.connection.clone(),
        entry.config.clone(),
        entry.schema.clone(),
    ))
}

pub fn list_connections() -> Vec<(String, ConnectionConfig)> {
    let registry = match CONNECTION_REGISTRY.read() {
        Ok(guard) => guard,
        Err(error) => {
            log::error!("Failed to acquire connection registry read lock: {error}");
            return Vec::new();
        }
    };
    registry
        .iter()
        .map(|(name, entry)| (name.clone(), entry.config.clone()))
        .collect()
}

pub fn update_connection_schema(name: &str, schema: DatabaseSchema) {
    match CONNECTION_REGISTRY.write() {
        Ok(mut registry) => {
            if let Some(entry) = registry.get_mut(name) {
                entry.schema = Some(schema);
            }
        }
        Err(error) => {
            log::error!("Failed to acquire connection registry write lock: {error}");
        }
    }
}

pub fn clear_all_connections() {
    match CONNECTION_REGISTRY.write() {
        Ok(mut registry) => {
            registry.clear();
        }
        Err(error) => {
            log::error!("Failed to acquire connection registry write lock: {error}");
        }
    }
}

pub fn connection_count() -> usize {
    match CONNECTION_REGISTRY.read() {
        Ok(registry) => registry.len(),
        Err(error) => {
            log::error!("Failed to acquire connection registry read lock: {error}");
            0
        }
    }
}

static MCP_SOCKET_PATH: LazyLock<RwLock<Option<PathBuf>>> =
    LazyLock::new(|| RwLock::new(None));

pub fn set_mcp_socket_path(path: Option<PathBuf>) {
    match MCP_SOCKET_PATH.write() {
        Ok(mut guard) => {
            *guard = path;
        }
        Err(error) => {
            log::error!("Failed to acquire MCP socket path write lock: {error}");
        }
    }
}

pub fn get_mcp_socket_path() -> Option<PathBuf> {
    match MCP_SOCKET_PATH.read() {
        Ok(guard) => guard.clone(),
        Err(error) => {
            log::error!("Failed to acquire MCP socket path read lock: {error}");
            None
        }
    }
}
