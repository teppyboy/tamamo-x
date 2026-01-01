use reqwest::blocking as reqwest;
use std::fs::{create_dir, File};
use std::path::Path;
use sha2::{Sha256, Digest};
use std::io::{self, Read};
use tracing::info;

use crate::HachimiVersion;

fn calculate_sha256<P: AsRef<Path>>(path: P) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0; 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn hachimi_download_latest(version: HachimiVersion) -> Result<String, String> {
    let mut should_check_sha256 = true;
    if !Path::new("external").exists() {
        let _ = create_dir("external");
        should_check_sha256 = false;
    }

    let file_name;
    match version {
        HachimiVersion::Original => {
            if !Path::new("external/hachimi").exists() {
                let _ = create_dir("external/hachimi");
                should_check_sha256 = false;
            }
            file_name = "external/hachimi/hachimi.dll";
        }
        HachimiVersion::Edge => {
            if !Path::new("external/hachimi-edge").exists() {
                let _ = create_dir("external/hachimi-edge");
                should_check_sha256 = false;
            }
            file_name = "external/hachimi-edge/hachimi.dll";
        }
    }

    let client = reqwest::Client::new();
    let api_url;
    match version {
        HachimiVersion::Original => {
            api_url = "https://api.github.com/repos/Hachimi-Hachimi/Hachimi/releases/latest";
        }
        HachimiVersion::Edge => {
            api_url = "https://api.github.com/repos/kairusds/Hachimi-Edge/releases/latest";
        }
    }
    
    let rsp = client
        .get(api_url)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .send()
        .map_err(|e| format!("Failed to get latest release: {}", e))?;

    let json: serde_json::Value = rsp.json().map_err(|e| format!("Failed to parse JSON: {}", e))?;
    let assets = json["assets"].as_array().ok_or("No assets found in release")?;
    
    for asset in assets {
        let name = asset["name"].as_str().ok_or("Asset name is not a string")?;
        if name != "hachimi.dll" {
            continue;
        }

        if should_check_sha256 && Path::new(file_name).exists() {
            info!("Checking existing file SHA256...");
            if let Some(digest) = asset["digest"].as_str() {
                if let Some(sha256_remote) = digest.strip_prefix("sha256:") {
                    if let Ok(existing_file_sha256) = calculate_sha256(file_name) {
                        if existing_file_sha256 == sha256_remote {
                            info!("Hachimi is already up-to-date.");
                            return Ok(file_name.to_string());
                        }
                    }
                }
            }
            info!("Hachimi is outdated or hash check failed. Downloading latest version...");
        }

        let url = asset["browser_download_url"].as_str().ok_or("No download URL found")?;
        let mut rsp = client
            .get(url)
            .send()
            .map_err(|e| format!("Failed to download 'hachimi.dll': {}", e))?;
        
        if Path::new(file_name).exists() {
            let _ = std::fs::remove_file(file_name);
        }

        let mut out = File::create(file_name).map_err(|e| format!("Failed to create file: {}", e))?;
        std::io::copy(&mut rsp, &mut out).map_err(|e| format!("Failed to write to file: {}", e))?;

        info!("Successfully downloaded latest Hachimi.");
        return Ok(file_name.to_string());
    }
    
    Err("Failed to find hachimi.dll in the latest release.".to_string())
}
