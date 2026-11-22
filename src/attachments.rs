//! Pictures and files, the way Teams carries them.
//!
//! Pictures live in the media store (AMS). A message shows one with an
//! `<img itemtype="http://schema.skype.com/AMSImage" src="{ams}/v1/objects/{id}/views/imgo">`
//! and lists the object id in `amsreferences`. Uploading is two calls: create the object with
//! read permission for the chat (`POST /v1/objects`, type `pish/image`), then put the bytes
//! (`PUT /v1/objects/{id}/content/imgpsh`). The store takes `Authorization: skype_token <t>`.
//!
//! Every other file lives in the sender's OneDrive, folder "Microsoft Teams Chat Files", and
//! the message carries a description of it in `properties.files` (a JSON string). We upload
//! and share through Microsoft Graph, then describe the file with the same fields the web
//! client writes. Downloading goes through Graph's `/shares/{link}` so files in other
//! people's OneDrive work too.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::api::{Teams, urlencode};

const GRAPH: &str = "https://graph.microsoft.com/v1.0";
pub const SCOPE_GRAPH: &str = "https://graph.microsoft.com/.default";
const CHAT_FILES_FOLDER: &str = "Microsoft Teams Chat Files";
/// Graph wants upload chunks in multiples of 320 KiB; 5 MiB is 16 of them.
const CHUNK: usize = 16 * 320 * 1024;
const AMS_CLIENT_VERSION: &str = "1415/1.0.0.2018090100";

#[derive(Debug, Clone, PartialEq)]
pub struct ImageRef {
    /// Media-store object id; the cache key for the downloaded picture.
    pub id: String,
    pub url: String,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileRef {
    pub name: String,
    pub kind: String,
    pub size: Option<u64>,
    /// A link that opens the file in the browser (sharing link, else its address).
    pub open_url: String,
}

/// A picture already in the media store, ready to be put into a message.
pub struct SentImage {
    pub id: String,
    pub format: String,
    pub width: f32,
    pub height: f32,
}

impl SentImage {
    pub fn html(&self, ams: &str) -> String {
        format!(
            "<p><img src=\"{ams}/v1/objects/{id}/views/imgo\" itemtype=\"http://schema.skype.com/AMSImage\" itemscope=\"{fmt}\" width=\"{w}\" height=\"{h}\" alt=\"image\" id=\"x_{id}\" itemid=\"{id}\"></p>",
            id = self.id,
            fmt = self.format,
            w = self.width,
            h = self.height,
        )
    }
}

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let key = format!("{name}=\"");
    let start = tag.find(&key)? + key.len();
    let len = tag[start..].find('"')?;
    Some(&tag[start..start + len])
}

/// Every `<img ...>` tag with its byte range in `html`.
fn img_tags(html: &str) -> Vec<(usize, usize, &str)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(off) = html[from..].find("<img") {
        let start = from + off;
        let Some(len) = html[start..].find('>') else { break };
        let end = start + len + 1;
        out.push((start, end, &html[start..end]));
        from = end;
    }
    out
}

fn is_ams_image(tag: &str) -> bool {
    tag.contains("schema.skype.com/AMSImage") || tag.contains("/v1/objects/")
}

/// A GIF picked in Teams' GIF search: a plain `<img>` linking to Giphy (or Tenor).
fn is_web_gif(tag: &str) -> bool {
    attr(tag, "src").is_some_and(|src| {
        let host = src.strip_prefix("https://").and_then(|r| r.split('/').next()).unwrap_or_default();
        host.ends_with("giphy.com") || host.ends_with("tenor.com") || (host.contains('.') && src.split('?').next().is_some_and(|p| p.ends_with(".gif")))
    })
}

fn is_picture_tag(tag: &str) -> bool {
    is_ams_image(tag) || is_web_gif(tag)
}

/// Whether a picture's address is Teams' own media store, which needs the Teams token; any
/// other site (Giphy) gets no credentials.
pub fn is_teams_media(url: &str) -> bool {
    let host = url.strip_prefix("https://").and_then(|r| r.split('/').next()).unwrap_or_default();
    host.ends_with(".teams.microsoft.com") || host.ends_with(".skype.com") || host.ends_with(".teams.cloud.microsoft")
}

/// The full-size picture behind a chat picture: the chat shows the media store's reduced
/// `imgo` view (a 3024×4032 photo comes as 600×800); `imgpsh_fullsize_anim` is the picture as
/// it was sent, animation included. `None` for a picture from elsewhere (a GIF site).
pub fn full_size_url(url: &str) -> Option<String> {
    if !is_teams_media(url) {
        return None;
    }
    let (base, _) = url.rsplit_once("/views/")?;
    Some(format!("{base}/views/imgpsh_fullsize_anim"))
}

pub fn images_in(html: &str) -> Vec<ImageRef> {
    img_tags(html)
        .into_iter()
        .filter(|(_, _, tag)| is_picture_tag(tag))
        .filter_map(|(_, _, tag)| {
            let url = attr(tag, "src")?.replace("&amp;", "&");
            let id = attr(tag, "itemid")
                .map(str::to_string)
                .or_else(|| {
                    url.split("/objects/")
                        .nth(1)
                        .and_then(|r| r.split('/').next())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| url.clone());
            let num = |k| attr(tag, k).and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);
            Some(ImageRef {
                id,
                url,
                width: num("width"),
                height: num("height"),
            })
        })
        .collect()
}

/// The HTML without its pictures (media store and GIFs), so their alt text isn't shown as text.
pub fn strip_ams_images(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut last = 0;
    for (start, end, tag) in img_tags(html) {
        if is_picture_tag(tag) {
            out.push_str(&html[last..start]);
            last = end;
        }
    }
    out.push_str(&html[last..]);
    out
}

/// `properties.files` is usually a JSON string, sometimes already an array.
pub fn files_in(prop: &Value) -> Vec<FileRef> {
    let list: Value = match prop {
        Value::String(s) if !s.trim().is_empty() => serde_json::from_str(s).unwrap_or(Value::Null),
        Value::Array(_) => prop.clone(),
        _ => Value::Null,
    };
    let s = |v: &Value, path: &[&str]| -> String {
        let mut cur = v;
        for k in path {
            cur = &cur[*k];
        }
        cur.as_str().unwrap_or("").to_string()
    };
    list.as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| {
            let name = [s(f, &["fileName"]), s(f, &["title"])]
                .into_iter()
                .find(|n| !n.is_empty())?;
            let open_url = [
                s(f, &["fileInfo", "shareUrl"]),
                s(f, &["fileInfo", "fileUrl"]),
                s(f, &["objectUrl"]),
            ]
            .into_iter()
            .find(|u| !u.is_empty())
            .unwrap_or_default();
            let kind = [s(f, &["fileType"]), s(f, &["type"])]
                .into_iter()
                .find(|k| !k.is_empty())
                .unwrap_or_default();
            Some(FileRef {
                name,
                kind,
                size: f["fileSize"].as_u64(),
                open_url,
            })
        })
        .collect()
}

pub fn is_picture(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp")
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file")
        .to_string()
}

fn ext_of(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Graph's `/shares/{id}` key: "u!" + unpadded base64url of the link.
fn share_key(url: &str) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let b = url.as_bytes();
    let mut out = String::from("u!");
    for chunk in b.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        let chars = chunk.len() + 1;
        for i in 0..chars {
            out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

fn fail(what: &str, resp: reqwest::blocking::Response) -> String {
    let status = resp.status();
    let body: String = resp.text().unwrap_or_default().chars().take(200).collect();
    format!("{what} failed ({status}). {body}")
}

/// Downloads a media-store picture with a Skype token. A free function so the worker can run
/// several at once without sharing the session.
pub fn fetch_image_with(
    http: &reqwest::blocking::Client,
    skype_token: &str,
    url: &str,
) -> Result<Vec<u8>, String> {
    let mut req = http.get(url);
    if is_teams_media(url) {
        req = req.header("Authorization", format!("skype_token {skype_token}"));
    }
    let resp = req.send().map_err(crate::api::unreachable("the picture's site"))?;
    if !resp.status().is_success() {
        return Err(fail("Loading the picture", resp));
    }
    resp.bytes().map(|b| b.to_vec()).map_err(|e| e.to_string())
}

/// Downloads a chat file through Graph into `folder`, never overwriting ("name (1).ext").
pub fn download_with(
    http: &reqwest::blocking::Client,
    graph_token: &str,
    file: &FileRef,
    folder: &Path,
    overwrite: bool,
) -> Result<PathBuf, String> {
    let url = format!("{GRAPH}/shares/{}/driveItem/content", share_key(&file.open_url));
    let resp = http
        .get(url)
        .bearer_auth(graph_token)
        .timeout(std::time::Duration::from_secs(600))
        .send()
        .map_err(crate::api::unreachable("OneDrive"))?;
    if !resp.status().is_success() {
        return Err(fail("Downloading", resp));
    }
    let bytes = resp.bytes().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    let path = if overwrite { folder.join(&file.name) } else { free_path(folder, &file.name) };
    std::fs::write(&path, &bytes).map_err(|e| format!("Couldn't save the file: {e}"))?;
    Ok(path)
}

impl Teams {
    /// Downloads a media-store picture.
    pub fn fetch_image(&mut self, url: &str) -> Result<Vec<u8>, String> {
        let token = self.skype_token()?;
        fetch_image_with(&self.http, &token, url)
    }

    /// Downloads a chat file into `folder`.
    pub fn download_file(&mut self, file: &FileRef, folder: &Path) -> Result<PathBuf, String> {
        let token = self.auth.token(SCOPE_GRAPH)?;
        download_with(&self.http, &token, file, folder, false)
    }

    /// Puts a picture into the media store, readable by the chat's members.
    pub fn upload_image(&mut self, chat_id: &str, path: &Path) -> Result<SentImage, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
        let (w, h) = image::load_from_memory(&bytes)
            .map(|i| (i.width() as f32, i.height() as f32))
            .unwrap_or((400.0, 300.0));
        // Teams shows sent pictures at most 250 px tall (and we keep them under 480 wide).
        let scale = (250.0 / h).min(480.0 / w).min(1.0);
        let name = file_name(path);
        let token = self.skype_token()?;
        let create = self
            .http
            .post(format!("{}/v1/objects", self.ams))
            .header("Authorization", format!("skype_token {token}"))
            .header("x-ms-client-version", AMS_CLIENT_VERSION)
            .json(&json!({
                "type": "pish/image",
                "permissions": { chat_id.split(';').next().unwrap_or(chat_id): ["read"] },
                "filename": name,
            }))
            .send()
            .map_err(crate::api::unreachable("Teams"))?;
        if !create.status().is_success() {
            return Err(fail("Uploading the picture", create));
        }
        let v: Value = create.json().map_err(|e| e.to_string())?;
        let id = v["id"].as_str().ok_or("Teams didn't name the uploaded picture")?.to_string();
        let put = self
            .http
            .put(format!("{}/v1/objects/{}/content/imgpsh", self.ams, urlencode(&id)))
            .header("Authorization", format!("skype_token {token}"))
            .header("x-ms-client-version", AMS_CLIENT_VERSION)
            .header("Content-Type", "application/octet-stream")
            .timeout(std::time::Duration::from_secs(600))
            .body(bytes)
            .send()
            .map_err(crate::api::unreachable("Teams"))?;
        if !put.status().is_success() {
            return Err(fail("Uploading the picture", put));
        }
        let format = match ext_of(&name).as_str() {
            "jpg" | "jpeg" => "jpeg".to_string(),
            "" => "png".to_string(),
            e => e.to_string(),
        };
        Ok(SentImage {
            id,
            format,
            width: (w * scale).round(),
            height: (h * scale).round(),
        })
    }

    /// Uploads a file to the person's "Microsoft Teams Chat Files" folder, shares it with
    /// their organisation, and returns its description for `properties.files`.
    pub fn upload_file(&mut self, path: &Path) -> Result<Value, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
        let name = file_name(path);
        let token = self.auth.token(SCOPE_GRAPH)?;
        let session = self
            .http
            .post(format!(
                "{GRAPH}/me/drive/root:/{}/{}:/createUploadSession",
                urlencode_path(CHAT_FILES_FOLDER),
                urlencode_path(&name)
            ))
            .bearer_auth(&token)
            .json(&json!({ "item": { "@microsoft.graph.conflictBehavior": "rename" } }))
            .send()
            .map_err(crate::api::unreachable("OneDrive"))?;
        if !session.status().is_success() {
            return Err(fail("Uploading the file", session));
        }
        let v: Value = session.json().map_err(|e| e.to_string())?;
        let upload_url = v["uploadUrl"].as_str().ok_or("OneDrive gave no upload address")?.to_string();

        let total = bytes.len();
        let mut item = Value::Null;
        let mut start = 0;
        loop {
            let end = (start + CHUNK).min(total);
            // The upload address is pre-authorised; it must not get our token.
            let resp = self
                .http
                .put(&upload_url)
                .header("Content-Range", format!("bytes {start}-{}/{total}", end.max(1) - 1))
                .header("Content-Length", (end - start).to_string())
                .timeout(std::time::Duration::from_secs(600))
                .body(bytes[start..end].to_vec())
                .send()
                .map_err(crate::api::unreachable("OneDrive"))?;
            let status = resp.status();
            if !status.is_success() {
                return Err(fail("Uploading the file", resp));
            }
            if status.as_u16() == 200 || status.as_u16() == 201 {
                item = resp.json().map_err(|e| e.to_string())?;
                break;
            }
            start = end;
            if start >= total {
                break;
            }
        }
        let item_id = item["id"].as_str().ok_or("OneDrive didn't return the uploaded file")?.to_string();
        let stored_name = item["name"].as_str().unwrap_or(&name).to_string();

        let details: Value = self
            .http
            .get(format!(
                "{GRAPH}/me/drive/items/{item_id}?$select=id,name,size,webUrl,sharepointIds,parentReference"
            ))
            .bearer_auth(&token)
            .send()
            .and_then(|r| r.json())
            .map_err(crate::api::unreachable("OneDrive"))?;
        let root: Value = self
            .http
            .get(format!("{GRAPH}/me/drive/root?$select=webUrl"))
            .bearer_auth(&token)
            .send()
            .and_then(|r| r.json())
            .map_err(crate::api::unreachable("OneDrive"))?;
        let share: Value = self
            .http
            .post(format!("{GRAPH}/me/drive/items/{item_id}/createLink"))
            .bearer_auth(&token)
            .json(&json!({ "type": "view", "scope": "organization" }))
            .send()
            .and_then(|r| r.json())
            .unwrap_or(Value::Null);

        let sp = &details["sharepointIds"];
        let site_url = sp["siteUrl"].as_str().unwrap_or("").trim_end_matches('/').to_string();
        let file_url = format!(
            "{}/{}/{}",
            root["webUrl"].as_str().unwrap_or("").trim_end_matches('/'),
            urlencode_path(CHAT_FILES_FOLDER),
            urlencode_path(&stored_name)
        );
        let share_url = share["link"]["webUrl"].as_str().unwrap_or("").to_string();
        let unique_id = sp["listItemUniqueId"].as_str().unwrap_or(&item_id).to_string();
        let kind = ext_of(&stored_name);
        Ok(json!({
            "@type": "http://schema.skype.com/File",
            "version": 2,
            "id": unique_id,
            "itemid": unique_id,
            "baseUrl": format!("{site_url}/"),
            "objectUrl": file_url,
            "openUrl": null,
            "type": kind,
            "title": stored_name,
            "fileName": stored_name,
            "fileType": kind,
            "fileSize": details["size"].as_u64().unwrap_or(total as u64),
            "state": "active",
            "providerData": "",
            "chicletBreadcrumbs": null,
            "botFileProperties": {},
            "isUploadError": null,
            "progressComplete": null,
            "permissionScope": "Organization",
            "fileInfo": {
                "itemId": item_id,
                "fileUrl": file_url,
                "siteUrl": format!("{site_url}/"),
                "serverRelativeUrl": "",
                "shareUrl": share_url,
                "shareId": share["shareId"].as_str().unwrap_or(""),
            },
            "fileChicletState": { "serviceName": "p2p", "state": "active" },
            "filePreview": { "previewUrl": "", "previewHeight": 0, "previewWidth": 0 },
            "sharepointIds": {
                "listId": sp["listId"],
                "listItemUniqueId": sp["listItemUniqueId"],
                "siteId": sp["siteId"],
                "siteUrl": sp["siteUrl"],
                "webId": sp["webId"],
                "driveId": details["parentReference"]["driveId"],
            },
            "publication": null,
            "site": null,
        }))
    }

    /// Uploads every attachment (pictures to the media store, the rest to OneDrive) and
    /// posts one message with the text and all of them.
    pub fn send_with_attachments(
        &mut self,
        chat_id: &str,
        text: &str,
        paths: &[PathBuf],
        client_id: &str,
    ) -> Result<(), String> {
        let mut images = Vec::new();
        let mut files = Vec::new();
        for p in paths {
            if is_picture(p) {
                images.push(self.upload_image(chat_id, p)?);
            } else {
                files.push(self.upload_file(p)?);
            }
        }
        self.send_message(chat_id, text, &images, &files, client_id)
    }
}

/// Percent-encodes one path segment (spaces too) for Graph and SharePoint URLs.
fn urlencode_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn free_path(folder: &Path, name: &str) -> PathBuf {
    let first = folder.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), format!(".{e}")),
        None => (name.to_string(), String::new()),
    };
    (1..)
        .map(|n| folder.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    const IMG: &str = r#"<p>hi</p><p><img src="https://eu-api.asm.skype.com/v1/objects/0-weu-d21-abc/views/imgo" itemtype="http://schema.skype.com/AMSImage" itemscope="png" width="350.5" height="250" alt="image" id="x_0-weu-d21-abc" itemid="0-weu-d21-abc"></p><p><img alt="😀" itemtype="http://schema.skype.com/Emoji"></p>"#;

    #[test]
    fn finds_media_store_pictures_only() {
        let imgs = images_in(IMG);
        assert_eq!(imgs.len(), 1);
        assert_eq!(imgs[0].id, "0-weu-d21-abc");
        assert_eq!(imgs[0].height, 250.0);
        let stripped = strip_ams_images(IMG);
        assert!(!stripped.contains("AMSImage"));
        let gif = r#"<p>ha</p><img src="https://media1.giphy.com/media/v1.X/IBhB/giphy.gif" width="250" height="250" alt="Letters GIF">"#;
        let found = images_in(gif);
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].width, found[0].height), (250.0, 250.0));
        assert!(!is_teams_media(&found[0].url), "no Teams token goes to Giphy");
        assert!(!strip_ams_images(gif).contains("Letters GIF"));
        assert!(is_teams_media("https://eu-prod.asyncgw.teams.microsoft.com/v1/objects/x/views/imgo"));
        assert_eq!(
            full_size_url("https://eu-prod.asyncgw.teams.microsoft.com/v1/objects/x/views/imgo").as_deref(),
            Some("https://eu-prod.asyncgw.teams.microsoft.com/v1/objects/x/views/imgpsh_fullsize_anim")
        );
        assert_eq!(full_size_url("https://media1.giphy.com/media/a/giphy.gif"), None);
        assert!(stripped.contains("Emoji"));
    }

    #[test]
    fn reads_file_descriptions() {
        let prop = Value::String(
            r#"[{"fileName":"a b.md","fileType":"md","fileSize":12,"fileInfo":{"shareUrl":"https://x/s","fileUrl":"https://x/f"}}]"#.into(),
        );
        let f = files_in(&prop);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].name, "a b.md");
        assert_eq!(f[0].open_url, "https://x/s");
        assert_eq!(f[0].size, Some(12));
        assert!(files_in(&Value::String("[]".into())).is_empty());
    }

    #[test]
    fn share_key_is_unpadded_base64url() {
        assert_eq!(share_key("https://a"), "u!aHR0cHM6Ly9h");
    }
}
