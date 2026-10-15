//! Matrix media downloaded with a size limit. The SDK reads a whole file into
//! memory, however large the server sends it, and the size a message gives
//! is only what its sender says. Talk streams the download instead, giving
//! up past a limit, and keeps the SDK's media cache.

use std::{io::Write, path::Path};

use matrix_sdk::{
    Client,
    media::{MediaFormat, MediaRequestParameters, store::IgnoreMediaRetentionPolicy},
    ruma::{
        MxcUri,
        api::{Metadata, client::authenticated_media},
        events::room::{EncryptedFile, MediaSource},
    },
};

/// Why media didn't come.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Failed {
    TooLarge,
    Download,
    Decrypt,
    Write,
}

/// The server Talk's own outgoing media is kept under until it's sent, as
/// the SDK names it.
const LOCAL_SERVER: &str = "send-queue.localhost";

/// `source`'s content, decrypted, from the media cache or the server, unless
/// it's larger than `max` bytes.
pub(crate) async fn content(
    client: &Client,
    source: &MediaSource,
    max: usize,
) -> Result<Vec<u8>, Failed> {
    let request = MediaRequestParameters {
        source: source.clone(),
        format: MediaFormat::File,
    };
    // Talk's own, still sending, is only in the store.
    if local(source) {
        return client
            .media()
            .get_media_content(&request, true)
            .await
            .map_err(|_| Failed::Download);
    }
    if let Ok(store) = client.media_store().lock().await
        && let Ok(Some(content)) = store.get_media_content(&request).await
    {
        return if content.len() <= max {
            Ok(content)
        } else {
            Err(Failed::TooLarge)
        };
    }
    let mut data = Vec::new();
    download(client, uri(source), max as u64, |chunk| {
        data.extend_from_slice(chunk);
        Ok(())
    })
    .await?;
    let content = match source {
        MediaSource::Plain(_) => data,
        MediaSource::Encrypted(file) => {
            let mut content = Vec::with_capacity(data.len());
            decrypt(std::io::Cursor::new(data), file, &mut content)?;
            content
        }
    };
    if let Ok(store) = client.media_store().lock().await {
        let _ = store
            .add_media_content(&request, content.clone(), IgnoreMediaRetentionPolicy::No)
            .await;
    }
    Ok(content)
}

/// Saves `source`, decrypted, to `path`, streaming it to disk, unless it's
/// larger than `max` bytes. Nothing is left at `path` when it fails.
pub(crate) async fn save(
    client: &Client,
    source: &MediaSource,
    max: u64,
    path: &Path,
) -> Result<(), Failed> {
    if local(source) {
        let content = content(client, source, usize::MAX).await?;
        return tokio::fs::write(path, content)
            .await
            .map_err(|_| Failed::Write);
    }
    let folder = path.parent().unwrap_or(Path::new("."));
    let mut downloaded = tempfile::NamedTempFile::new_in(folder).map_err(|_| Failed::Write)?;
    download(client, uri(source), max, |chunk| {
        downloaded.write_all(chunk)
    })
    .await?;
    let path = path.to_owned();
    let folder = folder.to_owned();
    let source = source.clone();
    tokio::task::spawn_blocking(move || {
        let file = match &source {
            MediaSource::Plain(_) => downloaded,
            MediaSource::Encrypted(info) => {
                let mut decrypted =
                    tempfile::NamedTempFile::new_in(&folder).map_err(|_| Failed::Write)?;
                let encrypted = downloaded.reopen().map_err(|_| Failed::Write)?;
                decrypt(std::io::BufReader::new(encrypted), info, &mut decrypted)?;
                decrypted
            }
        };
        file.persist(&path).map_err(|_| Failed::Write)?;
        Ok(())
    })
    .await
    .map_err(|_| Failed::Write)?
}

fn uri(source: &MediaSource) -> &MxcUri {
    match source {
        MediaSource::Plain(uri) => uri,
        MediaSource::Encrypted(file) => &file.url,
    }
}

fn local(source: &MediaSource) -> bool {
    uri(source)
        .server_name()
        .is_ok_and(|server| server == LOCAL_SERVER)
}

/// Streams `uri`'s bytes, as the server holds them, to `write`, giving up
/// once they pass `max`. The homeserver's authenticated endpoint takes the
/// access token, as the SDK's own downloads do; an older server's takes
/// none.
async fn download(
    client: &Client,
    uri: &MxcUri,
    max: u64,
    mut write: impl FnMut(&[u8]) -> std::io::Result<()>,
) -> Result<(), Failed> {
    let (server, media) = uri.parts().map_err(|_| Failed::Download)?;
    let authenticated = client.supported_versions().await.is_ok_and(|versions| {
        authenticated_media::get_content::v1::Request::PATH_BUILDER.is_supported(&versions)
    });
    let mut url = client.homeserver();
    {
        let mut path = url.path_segments_mut().map_err(|()| Failed::Download)?;
        path.pop_if_empty();
        if authenticated {
            path.extend(["_matrix", "client", "v1", "media", "download"]);
        } else {
            path.extend(["_matrix", "media", "v3", "download"]);
        }
        path.extend([server.as_str(), media]);
    }
    let mut request = client.http_client().get(url);
    if authenticated && let Some(token) = client.access_token() {
        request = request.bearer_auth(token);
    }
    let mut response = request
        .send()
        .await
        .and_then(|response| response.error_for_status())
        .map_err(|_| Failed::Download)?;
    if response.content_length().is_some_and(|length| length > max) {
        return Err(Failed::TooLarge);
    }
    let mut received = 0u64;
    while let Some(chunk) = response.chunk().await.map_err(|_| Failed::Download)? {
        received += chunk.len() as u64;
        if received > max {
            return Err(Failed::TooLarge);
        }
        write(&chunk).map_err(|_| Failed::Write)?;
    }
    Ok(())
}

/// Decrypts an encrypted attachment into `out`, checking it against the hash
/// its message gives.
fn decrypt(
    mut encrypted: impl std::io::Read,
    file: &EncryptedFile,
    out: &mut impl Write,
) -> Result<(), Failed> {
    let mut decryptor =
        matrix_sdk_crypto::AttachmentDecryptor::new(&mut encrypted, file.clone().into())
            .map_err(|_| Failed::Decrypt)?;
    std::io::copy(&mut decryptor, out).map_err(|_| Failed::Decrypt)?;
    Ok(())
}
