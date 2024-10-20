use anyhow::{Context, Result};
use dotenv_parser::parse_dotenv;
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Command;
use std::time::Duration;
use tempfile;
use tokio::fs as tokio_fs;
use tokio::process::Command as TokioCommand;
use tokio::time::timeout;
use toml_edit::{DocumentMut, Item, Table};

const EXTENSIONS_REPO: &str = "https://github.com/ronnakamoto/bunsan-extensions";
const EXTENSIONS_DIR: &str = "extensions";
const DIST_DIR: &str = "dist";
const ENV_EXAMPLE: &str = ".env.example";
const PACKAGE_JSON: &str = "package.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParameterSource {
    Body,
    Header,
    Query,
    Path,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParameterConfig {
    pub name: String,
    #[serde(rename = "type")]
    pub param_type: String,
    pub required: bool,
    pub description: Option<String>,
    pub source: ParameterSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteConfig {
    pub path: String,
    pub method: String,
    pub command: String,
    pub description: Option<String>,
    pub parameters: Option<Vec<ParameterConfig>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PackageJson {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    main: String,
    bunsan: Option<BunsanConfig>,
}

#[derive(Debug, Serialize, Deserialize)]
struct BunsanConfig {
    binary_name: Option<String>,
    routes: Option<Vec<RouteConfig>>,
}

#[derive(Debug, Clone)]
pub struct ExtensionManager {
    extensions_path: PathBuf,
    config_path: PathBuf,
    routes: HashMap<String, Vec<RouteConfig>>,
}

impl ExtensionManager {
    pub fn new(base_path: &Path, config_path: &Path) -> Self {
        let extensions_path = base_path.join(EXTENSIONS_DIR);
        fs::create_dir_all(&extensions_path).expect("Failed to create extensions directory");

        Self {
            extensions_path,
            config_path: config_path.to_path_buf(),
            routes: HashMap::new(),
        }
    }

    pub async fn load_extensions(&mut self) -> Result<()> {
        info!(
            "Starting to load extensions from: {:?}",
            self.extensions_path
        );
        let mut entries = tokio_fs::read_dir(&self.extensions_path).await?;
        while let Some(entry) = entries.next_entry().await? {
            if entry.file_type().await?.is_dir() {
                let package_json_path = entry.path();
                let package_json_file = package_json_path.join(PACKAGE_JSON);
                info!("Checking for package.json at: {:?}", package_json_file);
                if package_json_file.exists() {
                    info!("Found package.json, attempting to read");
                    match self.read_package_json(&package_json_path).await {
                        Ok(package_json) => {
                            info!("Successfully read package.json for: {}", package_json.name);
                            if let Some(bunsan_config) = &package_json.bunsan {
                                if let Some(routes) = &bunsan_config.routes {
                                    info!(
                                        "Loaded {} routes for extension: {}",
                                        routes.len(),
                                        package_json.name
                                    );
                                    for route in routes {
                                        info!("Route: {} {}", route.method, route.path);
                                    }
                                    self.routes
                                        .insert(package_json.name.clone(), routes.clone());
                                } else {
                                    warn!("No routes defined for extension: {}", package_json.name);
                                }
                            } else {
                                warn!(
                                    "No Bunsan configuration found for extension: {}",
                                    package_json.name
                                );
                            }
                        }
                        Err(e) => {
                            error!(
                                "Failed to load extension from {:?}: {}",
                                package_json_file, e
                            );
                        }
                    }
                } else {
                    warn!("No package.json found at: {:?}", package_json_file);
                }
            }
        }
        info!(
            "Finished loading extensions. Loaded {} extensions",
            self.routes.len()
        );
        Ok(())
    }

    pub fn get_all_routes(&self) -> &HashMap<String, Vec<RouteConfig>> {
        &self.routes
    }

    pub async fn install_extension(&mut self, extension_name: &str) -> Result<()> {
        info!("Starting installation of extension: {}", extension_name);
        let temp_dir = tempfile::tempdir().context("Failed to create temporary directory")?;
        let repo_path = temp_dir.path();
        debug!("Temporary directory created at: {:?}", repo_path);

        // Clone the extensions repository
        self.clone_repo(repo_path).await?;
        debug!("Extensions repository cloned to temporary directory");

        // List contents of the cloned repository
        self.list_directory_contents(repo_path).await?;

        // Check if the extension exists
        let extension_path = repo_path.join(extension_name);
        if !extension_path.exists() {
            warn!("Extension '{}' not found in the repository", extension_name);
            let available_extensions = self.list_available_extensions(repo_path).await?;
            error!("Available extensions: {:?}", available_extensions);
            anyhow::bail!(
                "Extension '{}' not found. Available extensions: {:?}",
                extension_name,
                available_extensions
            );
        }

        // Copy all files from the dist folder to the Bunsan extensions directory
        let src_dist_path = extension_path.join(DIST_DIR);
        let dest_path = self.extensions_path.join(extension_name);

        self.copy_dir_all(&src_dist_path, &dest_path).await?;
        info!("Copied all files from dist folder to: {:?}", dest_path);

        // Copy package.json to the extension directory
        let dest_package_json = dest_path.join(PACKAGE_JSON);
        tokio_fs::copy(extension_path.join(PACKAGE_JSON), &dest_package_json).await?;
        info!("Copied package.json to: {:?}", dest_package_json);

        // Read .env.example and update config.toml
        self.update_config_from_env_example(extension_name, &extension_path)
            .await?;
        info!("Updated config.toml with extension environment variables");

        // After successful installation, load the extension's routes
        let package_json = self.read_package_json(&dest_path).await?;
        if let Some(routes) = package_json.bunsan.and_then(|c| c.routes) {
            self.routes.insert(package_json.name.clone(), routes);
        }

        println!("Extension '{}' installed successfully", extension_name);
        println!(
            "Description: {}",
            package_json.description.unwrap_or_default()
        );
        println!("Version: {}", package_json.version);
        println!("Installed at: {:?}", dest_path);

        info!("Extension '{}' installation completed", extension_name);
        // The temporary directory will be automatically deleted when it goes out of scope
        Ok(())
    }

    fn copy_dir_all<'a>(
        &'a self,
        src: impl AsRef<Path> + 'a,
        dst: impl AsRef<Path> + 'a,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
        Box::pin(async move {
            tokio_fs::create_dir_all(&dst).await?;
            let mut entries = tokio_fs::read_dir(src).await?;
            while let Some(entry) = entries.next_entry().await? {
                let ty = entry.file_type().await?;
                if ty.is_dir() {
                    self.copy_dir_all(entry.path(), dst.as_ref().join(entry.file_name()))
                        .await?;
                } else {
                    tokio_fs::copy(entry.path(), dst.as_ref().join(entry.file_name())).await?;
                }
            }
            Ok(())
        })
    }

    async fn clone_repo(&self, path: &Path) -> Result<()> {
        info!("Cloning repository from: {}", EXTENSIONS_REPO);

        const MAX_RETRIES: u32 = 3;
        const TIMEOUT_SECONDS: u64 = 60;

        for attempt in 1..=MAX_RETRIES {
            info!("Clone attempt {} of {}", attempt, MAX_RETRIES);

            let clone_result = timeout(
                Duration::from_secs(TIMEOUT_SECONDS),
                TokioCommand::new("git")
                    .args(&[
                        "clone",
                        "--depth",
                        "1",
                        EXTENSIONS_REPO,
                        path.to_str().unwrap(),
                    ])
                    .output(),
            )
            .await;

            match clone_result {
                Ok(Ok(output)) => {
                    if output.status.success() {
                        info!("Repository cloned successfully");
                        return Ok(());
                    } else {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        error!("Git clone failed: {}", stderr);
                        if attempt == MAX_RETRIES {
                            anyhow::bail!(
                                "Failed to clone extensions repository after {} attempts: {}",
                                MAX_RETRIES,
                                stderr
                            );
                        }
                    }
                }
                Ok(Err(e)) => {
                    error!("Error executing git command: {}", e);
                    if attempt == MAX_RETRIES {
                        anyhow::bail!(
                            "Failed to execute git command after {} attempts: {}",
                            MAX_RETRIES,
                            e
                        );
                    }
                }
                Err(_) => {
                    error!(
                        "Git clone operation timed out after {} seconds",
                        TIMEOUT_SECONDS
                    );
                    if attempt == MAX_RETRIES {
                        anyhow::bail!(
                            "Git clone operation timed out after {} attempts",
                            MAX_RETRIES
                        );
                    }
                }
            }

            // Wait before retrying
            tokio::time::sleep(Duration::from_secs(5)).await;
        }

        unreachable!("This point should never be reached due to the for loop's logic");
    }
    async fn list_directory_contents(&self, path: &Path) -> Result<()> {
        info!("Listing contents of directory: {:?}", path);
        let mut entries = tokio_fs::read_dir(path).await?;
        while let Some(entry) = entries.next_entry().await? {
            let file_name = entry.file_name();
            let file_type = entry.file_type().await?;
            info!("{}: {:?}", file_name.to_string_lossy(), file_type);
        }
        Ok(())
    }

    async fn list_available_extensions(&self, repo_path: &Path) -> Result<Vec<String>> {
        let mut extensions = Vec::new();
        let mut entries = tokio_fs::read_dir(repo_path).await?;
        while let Some(entry) = entries.next_entry().await? {
            if entry.file_type().await?.is_dir() {
                extensions.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        Ok(extensions)
    }

    async fn read_package_json(&self, extension_path: &Path) -> Result<PackageJson> {
        let package_json_path = extension_path.join(PACKAGE_JSON);
        info!(
            "Attempting to read package.json from: {:?}",
            package_json_path
        );

        let package_json_content = tokio_fs::read_to_string(&package_json_path)
            .await
            .with_context(|| {
                format!(
                    "Failed to read package.json file at {:?}",
                    package_json_path
                )
            })?;

        info!("Successfully read package.json content");
        debug!("package.json content: {}", package_json_content);

        let package_json: PackageJson =
            serde_json::from_str(&package_json_content).with_context(|| {
                format!(
                    "Failed to parse package.json content from {:?}",
                    package_json_path
                )
            })?;

        info!("Successfully parsed package.json");
        Ok(package_json)
    }

    async fn update_config_from_env_example(
        &self,
        extension_name: &str,
        extension_path: &Path,
    ) -> Result<()> {
        let env_example_path = extension_path.join(ENV_EXAMPLE);
        let env_content = tokio_fs::read_to_string(env_example_path).await?;
        let env_vars = parse_dotenv(&env_content)
            .map_err(|e| anyhow::anyhow!("Failed to parse .env file: {}", e))?;

        let config_content = tokio_fs::read_to_string(&self.config_path).await?;
        let mut doc = config_content.parse::<DocumentMut>()?;

        let extensions = doc["extensions"].or_insert(Item::Table(Table::new()));
        let extension_table = extensions
            .as_table_mut()
            .expect("extensions should be a table")
            .entry(extension_name)
            .or_insert(Item::Table(Table::new()))
            .as_table_mut()
            .expect("extension entry should be a table");

        extension_table.insert("name", toml_edit::value(extension_name));
        for (key, value) in env_vars {
            extension_table.insert(&key, toml_edit::value(value));
        }

        tokio_fs::write(&self.config_path, doc.to_string()).await?;

        Ok(())
    }

    pub async fn list_installed_extensions(&self) -> Result<Vec<PackageJson>> {
        let mut installed_extensions = Vec::new();

        let mut entries = tokio_fs::read_dir(&self.extensions_path).await?;
        while let Some(entry) = entries.next_entry().await? {
            if entry.file_type().await?.is_dir() {
                let package_json_path = entry.path().join(PACKAGE_JSON);
                if package_json_path.exists() {
                    let package_json = self.read_package_json(&entry.path()).await?;
                    installed_extensions.push(package_json);
                }
            }
        }

        Ok(installed_extensions)
    }

    pub async fn uninstall_extension(&mut self, extension_name: &str) -> Result<()> {
        let extension_path = self.extensions_path.join(extension_name);
        if !extension_path.exists() {
            anyhow::bail!("Extension '{}' is not installed", extension_name);
        }

        // Remove the extension's routes
        self.routes.remove(extension_name);

        tokio_fs::remove_dir_all(extension_path).await?;

        // Remove extension from config.toml
        let config_content = tokio_fs::read_to_string(&self.config_path).await?;
        let mut doc = config_content.parse::<DocumentMut>()?;

        if let Some(extensions) = doc["extensions"].as_table_mut() {
            extensions.remove(extension_name);
        }

        tokio_fs::write(&self.config_path, doc.to_string()).await?;

        println!("Extension '{}' uninstalled successfully", extension_name);
        Ok(())
    }

    pub async fn run_extension(
        &self,
        extension_name: &str,
        command: &str,
        args: &[String],
        route: Option<&RouteConfig>,
        headers: Option<&HashMap<String, String>>,
        body: Option<&serde_json::Value>,
        query_params: Option<&HashMap<String, String>>,
        path_params: Option<&HashMap<String, String>>,
    ) -> Result<String> {
        info!("Starting run_extension for: {}", extension_name);
        let extension_path = self.extensions_path.join(extension_name);
        if !extension_path.exists() {
            error!("Extension '{}' is not installed", extension_name);
            anyhow::bail!("Extension '{}' is not installed", extension_name);
        }

        info!("Reading package.json for extension: {}", extension_name);
        let package_json = self.read_package_json(&extension_path).await?;
        let binary_name = package_json
            .bunsan
            .as_ref()
            .and_then(|config| config.binary_name.clone())
            .unwrap_or_else(|| package_json.name.clone());

        info!("Binary name for extension: {}", binary_name);
        let binary_path = extension_path.join(&binary_name);

        if !binary_path.exists() {
            error!(
                "Binary not found for extension '{}' at {:?}",
                extension_name, binary_path
            );
            anyhow::bail!("Binary not found for extension '{}'", extension_name);
        }

        // Prepare CLI arguments
        let mut cli_args = vec![command.to_string()];
        cli_args.extend_from_slice(args);

        // If route is provided, add parameters from the route configuration
        if let Some(route) = route {
            info!("Processing route configuration: {:?}", route);
            if let Some(parameters) = &route.parameters {
                for param in parameters {
                    info!("Processing parameter: {:?}", param);
                    let value = match param.source {
                        ParameterSource::Body => body.and_then(|b| {
                            b.get(&param.name)
                                .and_then(|v| v.as_str())
                                .map(String::from)
                        }),
                        ParameterSource::Header => {
                            headers.and_then(|h| h.get(&param.name).cloned())
                        }
                        ParameterSource::Query => {
                            query_params.and_then(|q| q.get(&param.name).cloned())
                        }
                        ParameterSource::Path => {
                            path_params.and_then(|p| p.get(&param.name).cloned())
                        }
                    };

                    match value {
                        Some(v) => {
                            cli_args.push(format!("--{}", param.name));
                            cli_args.push(v.clone());
                            info!("Added parameter: {} = {}", param.name, v);
                        }
                        None if param.required => {
                            error!("Required parameter '{}' is missing", param.name);
                            anyhow::bail!("Required parameter '{}' is missing", param.name);
                        }
                        None => {
                            info!("Optional parameter '{}' not provided", param.name);
                        }
                    }
                }
            } else {
                info!("No parameters defined for this route");
            }
        } else {
            info!("No route configuration provided");
        }

        info!("Final CLI args: {:?}", cli_args);

        // Read extension-specific environment variables from config.toml
        let config_content = tokio_fs::read_to_string(&self.config_path).await?;
        let doc = config_content.parse::<DocumentMut>()?;
        let mut env_vars = std::collections::HashMap::new();
        if let Some(extensions) = doc["extensions"].as_table() {
            if let Some(ext_config) = extensions.get(extension_name) {
                if let Some(ext_table) = ext_config.as_table() {
                    for (key, value) in ext_table.iter() {
                        if key != "name" {
                            env_vars.insert(
                                key.to_string(),
                                value.as_str().unwrap_or_default().to_string(),
                            );
                            info!(
                                "Added env var: {} = {}",
                                key,
                                value.as_str().unwrap_or_default()
                            );
                        }
                    }
                }
            }
        }

        info!(
            "Executing binary: {:?} with args: {:?}",
            binary_path, cli_args
        );
        let output = Command::new(&binary_path)
            .args(&cli_args)
            .envs(&env_vars)
            .output()
            .context("Failed to run extension")?;

        if !output.status.success() {
            let error_message = String::from_utf8_lossy(&output.stderr);
            error!(
                "Extension '{}' failed with status {:?}: {}",
                extension_name, output.status, error_message
            );
            anyhow::bail!(
                "Extension '{}' failed with status {:?}: {}",
                extension_name,
                output.status,
                error_message
            );
        }

        let output_message = String::from_utf8_lossy(&output.stdout);
        info!(
            "Extension '{}' completed successfully. Output: {}",
            extension_name, output_message
        );
        Ok(output_message.to_string())
    }
}
