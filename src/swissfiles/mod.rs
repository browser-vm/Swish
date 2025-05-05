use std::{fmt, path::PathBuf};

mod swissfile;
pub mod uploadparameters;
use crate::{
    api::{get, post},
    errors::SwishError,
    swissfiles::swissfile::{RemoteSwissfile, Swissfile},
};
use base64::prelude::*;
use serde_json::json;

use self::uploadparameters::UploadParameters;

const SWISSTRANSFER_API: &str = "https://www.swisstransfer.com/api";

pub struct Swissfiles {
    pub files: Vec<Swissfile>,
    pub container_uuid: String,
}

impl Swissfiles {
    pub fn new_remotefiles(
        swisstransfer_link: &str,
        password: Option<&str>,
    ) -> Result<Self, SwishError> {
        log::debug!("Creating new swissfiles : {}", &swisstransfer_link);

        // We might verify link validity there idk

        let download_id = swisstransfer_link.split("/").last().unwrap();
        let url = format!("{}/links/{}", SWISSTRANSFER_API, download_id);

        // if password is provided, add authorization header
        let auth_header: Option<Vec<String>> = match password {
            Some(password) => {
                // Encode password to base64
                let password = BASE64_STANDARD.encode(password);

                // Add authorization header
                let header = "Authorization: ".to_string() + &password;

                log::debug!(
                    "Password has been provided. Adding authorization header : {}",
                    &header
                );
                Some(vec![header])
            }
            None => None,
        };

        let response = get(&url, auth_header.clone())?;
        let response: serde_json::Value = serde_json::from_str(&response)?;

        //  Handling different responses
        match response["data"]["message"].as_str() {
            Some("Transfer need a password") => return Err(SwishError::PasswordRequired),
            Some("The password is wrong") => return Err(SwishError::InvalidPassword),
            Some("All file are not finish to virus check") => {
                loop {
                    // Wait for security checks on Infomaniak's side
                    print!("Waiting for security checks on Infomaniak's side");
                    std::thread::sleep(std::time::Duration::from_secs(5));
                    let response = get(&url, auth_header.clone())?;
                    let response: serde_json::Value = serde_json::from_str(&response)?;

                    let message = response["data"]["message"].as_str();

                    if message == Some("All file are finish to virus check") {
                        break;
                    }
                }
            }
            //Everything went probably fine
            _ => (),
        }

        //Retrieve information from the resposne
        let download_host = response["data"]["downloadHost"]
            .as_str()
            .unwrap()
            .to_string();
        let link_uuid = response["data"]["linkUUID"].as_str().unwrap().to_string();
        let container_uuid = response["data"]["container"]["UUID"]
            .as_str()
            .unwrap()
            .to_string();

        // Build the download base url
        let download_base_url = format!("https://{}/api/download/{}", &download_host, &link_uuid);

        let mut files = Vec::new();

        for file in response["data"]["container"]["files"].as_array().unwrap() {
            // We should probably generate the download token here and pass it to the Swissfile constructor
            let swissfile = Swissfile::Remote(RemoteSwissfile::new(
                file,
                &download_base_url,
                &container_uuid,
                password,
            ));
            files.push(swissfile);
        }

        let swissfiles = Swissfiles {
            files,
            container_uuid,
        };

        Ok(swissfiles)
    }

    pub fn new_localfiles(
        path: PathBuf,
        upload_parameter: &UploadParameters,
    ) -> Result<Self, SwishError> {
        // Get all files in the directory or just the single file
        let files = if path.is_dir() {
            std::fs::read_dir(path)
                .map_err(|e| SwishError::FileError { error: e })?
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path())
                .filter(|p| p.is_file()) // Only include files, not subdirectories
                .collect::<Vec<_>>()
        } else {
            vec![path]
        };

        // Early return if no files found
        if files.is_empty() {
            return Err(SwishError::FileError {
                error: std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "No files found in directory",
                ),
            });
        }

        // Get the container with file information
        let container = get_container(&files, upload_parameter)?;

        // Debug output
        log::debug!(
            "Container response: {}",
            serde_json::to_string(&container).unwrap_or_default()
        );

        let mut swissfiles = Vec::new();

        // Match files to their UUIDs based on name and size
        if let Some(files_uuid) = container.get("filesUUID").and_then(|v| v.as_array()) {
            // Create a mapping between file information and UUIDs
            for (index, file_path) in files.iter().enumerate() {
                if index >= files_uuid.len() {
                    log::warn!("More files than UUIDs available");
                    break;
                }

                let file_uuid = files_uuid[index].as_str().unwrap_or_default().to_string();
                let swissfile = Swissfile::Local(swissfile::LocalSwissfile::new(
                    file_path.clone(),
                    &container,
                    file_uuid,
                ));
                swissfiles.push(swissfile);
            }
        } else {
            return Err(SwishError::InvalidJson {
                json: "Missing filesUUID in container response".to_string(),
            });
        }

        Ok(Swissfiles {
            files: swissfiles,
            container_uuid: container["container"]["UUID"].as_str().unwrap().to_string(),
        })
    }
    pub fn download(&self, custom_out_path: Option<&PathBuf>) -> Result<(), SwishError> {
        // Create the directory if it doesn't exist or use the current directory
        let out_path = match custom_out_path {
            Some(path) => {
                if !path.exists() {
                    Some(std::fs::create_dir_all(path)?);
                }
                Some(path)
            }
            None => None,
        };

        for file in &self.files {
            match file {
                Swissfile::Local(_) => {
                    // Handle local file download
                    unimplemented!("Humm, Why would you want to download a local file ?")
                }
                Swissfile::Remote(remote_swissfile) => {
                    // Call download method on RemoteSwissfile
                    remote_swissfile.download(out_path)?;
                }
            }
        }
        Ok(())
    }

    pub fn upload(&self) -> Result<String, SwishError> {
        for file in &self.files {
            match file {
                Swissfile::Local(local_swissfile) => {
                    // Call upload method on LocalSwissfile
                    local_swissfile.upload()?;
                }
                Swissfile::Remote(_) => {
                    // Handle remote file upload
                    unimplemented!("Humm, Why would you want to upload a remote file ?")
                }
            }
        }

        let download_link = self.finalize_upload()?;
        Ok(download_link)
    }

    fn finalize_upload(&self) -> Result<String, SwishError> {
        let url = format!("{}/uploadComplete", SWISSTRANSFER_API);
        let body = json!({
            "UUID": self.container_uuid,
            "lang": "en_GB"
        })
        .to_string()
        .into_bytes();
        let response = post(&url, body, None)?;

        Ok(create_download_link(&response)?)
    }
}

impl fmt::Display for Swissfiles {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        for file in &self.files {
            write!(f, "{}\n", file)?;
        }
        Ok(())
    }
}

fn get_container(
    files: &[PathBuf],
    upload_parameter: &UploadParameters,
) -> Result<serde_json::Value, SwishError> {
    let url = format!("{}/containers", SWISSTRANSFER_API);

    let files_json: Vec<_> = files
        .iter()
        .filter(|file| file.is_file())
        .map(|file| {
            let name = file
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
            let size = file.metadata().map(|m| m.len()).unwrap_or(0);

            json!({
                "name": name,
                "size": size
            })
        })
        .collect();

    // Early return if no valid files
    if files_json.is_empty() {
        return Err(SwishError::FileError {
            error: std::io::Error::new(std::io::ErrorKind::NotFound, "No valid files to upload"),
        });
    }

    let files_string = serde_json::to_string(&files_json).unwrap();

    // Calculate total upload size
    let total_size = files_json
        .iter()
        .map(|file| file["size"].as_u64().unwrap_or(0))
        .sum::<u64>();

    let payload = json!({
        "duration": upload_parameter.duration,
        "authorEmail": upload_parameter.author_email,
        "password": upload_parameter.password,
        "message": upload_parameter.message,
        "sizeOfUpload": total_size,
        "numberOfDownload": upload_parameter.number_of_download,
        "numberOfFile": files_json.len(),
        "lang": upload_parameter.lang,
        "recaptcha": "nope",
        "files": files_string,
        "recipientsEmails": "[]"
    });

    let payload_string = serde_json::to_string(&payload).unwrap();
    let payload_bytes = payload_string.as_bytes();

    let response = post(url.as_str(), payload_bytes.to_vec(), None)?;

    let response_str = String::from_utf8(response).map_err(|_| SwishError::InvalidResponse {
        response: "Invalid UTF-8 in container response".to_string(),
    })?;

    let response_json: serde_json::Value =
        serde_json::from_str(&response_str).map_err(|e| SwishError::InvalidJson {
            json: format!(
                "Failed to parse container JSON: {} - Response: {}",
                e, response_str
            ),
        })?;

    Ok(response_json)
}

pub fn create_download_link(response: &Vec<u8>) -> Result<String, SwishError> {
    // Convert u8 to string first to inspect the response
    let response_str =
        String::from_utf8(response.to_vec()).map_err(|e| SwishError::InvalidResponse {
            response: format!("Invalid UTF-8 in response: {}", e),
        })?;

    log::debug!("Download link response: {}", response_str);

    // Parse the JSON
    let response_json: serde_json::Value =
        serde_json::from_str(&response_str).map_err(|e| SwishError::InvalidJson {
            json: format!("Failed to parse JSON: {} - Response: {}", e, response_str),
        })?;

    // Check if we have an array or a single object
    let link_uuid = if response_json.is_array() && !response_json.as_array().unwrap().is_empty() {
        response_json[0]["linkUUID"].as_str()
    } else {
        response_json["linkUUID"].as_str()
    };

    // Safely extract the linkUUID with error handling
    let link_uuid = link_uuid.ok_or_else(|| SwishError::InvalidJson {
        json: format!("Missing linkUUID in response: {}", response_str),
    })?;

    let link = format!("https://www.swisstransfer.com/d/{}", link_uuid);

    Ok(link)
}
