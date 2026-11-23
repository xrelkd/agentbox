use std::{
    env,
    path::{Path, PathBuf},
};

use crate::{
    app::App, cli::error::Error, config::Config, error::Result, workspace::ProjectIdentity,
};

pub struct AppContext {
    pub home: PathBuf,
    pub config_path: PathBuf,
}

impl AppContext {
    pub fn new(explicit: Option<&Path>) -> Result<Self> {
        let home = env::var_os("HOME").map(PathBuf::from).ok_or(Error::HomeNotFound)?;
        let config_path = Config::resolve_path(explicit, &home);
        Ok(Self { home, config_path })
    }

    pub fn load_app(&self) -> Result<App> {
        let config = Config::load(&self.config_path)?;
        let identity = ProjectIdentity::resolve(&self.home, &config)?;
        Ok(App::new(config, identity, self.home.clone(), self.config_path.clone()))
    }
}
