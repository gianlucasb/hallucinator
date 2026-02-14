use axum::extract::Multipart;

/// The type of uploaded file.
#[derive(Debug)]
pub enum FileType {
    Pdf,
    Zip,
    TarGz,
}

/// An uploaded file with its data and metadata.
pub struct UploadedFile {
    pub filename: String,
    pub data: Vec<u8>,
    pub file_type: FileType,
}

/// Parsed form fields from the multipart upload.
pub struct FormFields {
    pub file: UploadedFile,
    pub openalex_key: Option<String>,
    pub s2_api_key: Option<String>,
    pub check_openalex_authors: bool,
    pub disabled_dbs: Vec<String>,
}

/// Parse a multipart form upload into structured form fields.
pub async fn parse_multipart(mut multipart: Multipart) -> Result<FormFields, String> {
    let mut file: Option<UploadedFile> = None;
    let mut openalex_key: Option<String> = None;
    let mut s2_api_key: Option<String> = None;
    let mut check_openalex_authors = false;
    let mut disabled_dbs: Vec<String> = Vec::new();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| format!("Failed to read form field: {}", e))?
    {
        let name = field.name().unwrap_or("").to_string();

        match name.as_str() {
            "pdf" => {
                let filename = field.file_name().unwrap_or("upload.pdf").to_string();
                let data = field
                    .bytes()
                    .await
                    .map_err(|e| format!("Failed to read file data: {}", e))?
                    .to_vec();

                let file_type = detect_file_type(&filename, &data)?;

                file = Some(UploadedFile {
                    filename,
                    data,
                    file_type,
                });
            }
            "openalex_key" => {
                let val = field
                    .text()
                    .await
                    .map_err(|e| format!("Failed to read openalex_key: {}", e))?;
                if !val.is_empty() {
                    openalex_key = Some(val);
                }
            }
            "s2_api_key" => {
                let val = field
                    .text()
                    .await
                    .map_err(|e| format!("Failed to read s2_api_key: {}", e))?;
                if !val.is_empty() {
                    s2_api_key = Some(val);
                }
            }
            "check_openalex_authors" => {
                let val = field
                    .text()
                    .await
                    .map_err(|e| format!("Failed to read check_openalex_authors: {}", e))?;
                check_openalex_authors = val == "true";
            }
            "disabled_dbs" => {
                let val = field
                    .text()
                    .await
                    .map_err(|e| format!("Failed to read disabled_dbs: {}", e))?;
                if !val.is_empty()
                    && let Ok(dbs) = serde_json::from_str::<Vec<String>>(&val)
                {
                    disabled_dbs = dbs;
                }
            }
            _ => {
                // Ignore unknown fields
                let _ = field.bytes().await;
            }
        }
    }

    let file = file.ok_or("No file uploaded")?;

    Ok(FormFields {
        file,
        openalex_key,
        s2_api_key,
        check_openalex_authors,
        disabled_dbs,
    })
}

/// Detect file type from extension and magic bytes.
fn detect_file_type(filename: &str, data: &[u8]) -> Result<FileType, String> {
    let lower = filename.to_lowercase();

    if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
        return Ok(FileType::TarGz);
    }
    if lower.ends_with(".zip") {
        return Ok(FileType::Zip);
    }
    if lower.ends_with(".pdf") {
        // Verify PDF magic bytes
        if !data.starts_with(b"%PDF-") {
            return Err("File has .pdf extension but doesn't appear to be a valid PDF".to_string());
        }
        return Ok(FileType::Pdf);
    }

    // Try detecting by magic bytes
    if data.starts_with(b"%PDF-") {
        return Ok(FileType::Pdf);
    }
    if data.starts_with(b"PK") {
        return Ok(FileType::Zip);
    }
    if data.len() >= 2 && data[0] == 0x1f && data[1] == 0x8b {
        return Ok(FileType::TarGz);
    }

    Err("Unsupported file type. Please upload a PDF, ZIP, or tar.gz file.".to_string())
}
