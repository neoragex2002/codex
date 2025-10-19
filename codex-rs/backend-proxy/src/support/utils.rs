use std::fs::File;
use std::fs::{self};
use std::io::Write;
use std::path::Path;

use anyhow::Result;
use serde::Serialize;

#[derive(Serialize)]
struct ServerInfo {
    port: u16,
    pid: u32,
}

pub fn write_server_info(path: &Path, port: u16) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }

    let info = ServerInfo {
        port,
        pid: std::process::id(),
    };
    let mut data = serde_json::to_string(&info)?;
    data.push('\n');
    let mut f = File::create(path)?;
    f.write_all(data.as_bytes())?;
    Ok(())
}
