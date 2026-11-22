//! Presence (Available, Busy, Away...) and profile pictures.
//!
//! Presence comes from the unified presence service the web client uses
//! (`{ups}/v1/presence/getpresence/`, token audience presence.teams.microsoft.com); my own
//! status is set with `v1/me/forceavailability/` (PUT a status, DELETE to go back to
//! automatic). Pictures come from the middle tier's `beta/users/{mri}/profilepicturev2`.

use std::collections::HashMap;

use serde_json::{Value, json};

use crate::api::Teams;

pub const SCOPE_PRESENCE: &str = "https://presence.teams.microsoft.com/.default";
/// How the web client names itself to the presence service ("Teams-V2-" + device type).
const CLIENT_AGENT: &str = "Teams-V2-Desktop";
const GATEWAY: &str = "https://teams.cloud.microsoft/api";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Availability {
    Available,
    Busy,
    DoNotDisturb,
    BeRightBack,
    Away,
    Offline,
    Unknown,
}

impl Availability {
    pub fn parse(s: &str) -> Availability {
        match s {
            "Available" | "AvailableIdle" => Availability::Available,
            "Busy" | "BusyIdle" | "InACall" | "InAMeeting" | "Presenting" => Availability::Busy,
            "DoNotDisturb" | "Focusing" => Availability::DoNotDisturb,
            "BeRightBack" => Availability::BeRightBack,
            "Away" => Availability::Away,
            "Offline" | "PresenceUnknown" => Availability::Offline,
            _ => Availability::Unknown,
        }
    }

    /// The value the presence service takes when I set my own status.
    pub fn as_api(self) -> &'static str {
        match self {
            Availability::Available => "Available",
            Availability::Busy => "Busy",
            Availability::DoNotDisturb => "DoNotDisturb",
            Availability::BeRightBack => "BeRightBack",
            Availability::Away => "Away",
            Availability::Offline => "Offline",
            Availability::Unknown => "Available",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Availability::Available => "Available",
            Availability::Busy => "Busy",
            Availability::DoNotDisturb => "Do not disturb",
            Availability::BeRightBack => "Be right back",
            Availability::Away => "Away",
            Availability::Offline => "Appear offline",
            Availability::Unknown => "Unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Presence {
    pub availability: Availability,
    /// What the person is doing when it says more than the availability ("InAMeeting").
    pub activity: String,
    /// An out-of-office or status note, if set.
    pub note: String,
}

impl Teams {
    /// Presence for up to a few hundred people at once.
    pub fn presence_of(&mut self, mris: &[String]) -> Result<HashMap<String, Presence>, String> {
        let mut out = HashMap::new();
        for batch in mris.chunks(100) {
            let token = self.auth.token(SCOPE_PRESENCE)?;
            let body: Vec<Value> = batch.iter().map(|m| json!({ "mri": m })).collect();
            let resp = self
                .http
                .post(format!("{}/v1/presence/getpresence/", self.presence))
                .bearer_auth(token)
                .json(&body)
                .send()
                .map_err(crate::api::unreachable("Teams"))?;
            let status = resp.status();
            let text = resp.text().unwrap_or_default();
            if !status.is_success() {
                return Err(format!("Loading presence failed ({status}). {}", text.chars().take(160).collect::<String>()));
            }
            let v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            for p in v.as_array().into_iter().flatten() {
                let Some(mri) = p["mri"].as_str() else { continue };
                let pr = &p["presence"];
                let note = pr["outOfOfficeNote"]["message"]
                    .as_str()
                    .or_else(|| pr["note"]["message"].as_str())
                    .unwrap_or("")
                    .to_string();
                out.insert(
                    mri.to_string(),
                    Presence {
                        availability: Availability::parse(pr["availability"].as_str().unwrap_or("")),
                        activity: pr["activity"].as_str().unwrap_or("").to_string(),
                        note,
                    },
                );
            }
        }
        Ok(out)
    }

    /// Sets my status, or with `None` goes back to automatic (from activity and calendar).
    pub fn set_my_availability(&mut self, availability: Option<Availability>) -> Result<(), String> {
        let token = self.auth.token(SCOPE_PRESENCE)?;
        let url = format!("{}/v1/me/forceavailability/", self.presence);
        let req = match availability {
            Some(a) => self.http.put(url).json(&json!({ "availability": a.as_api() })),
            None => self.http.delete(url),
        };
        let resp = req.bearer_auth(token).send().map_err(crate::api::unreachable("Teams"))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            let status = resp.status();
            Err(format!("Setting your status failed ({status}). {}", resp.text().unwrap_or_default().chars().take(160).collect::<String>()))
        }
    }

    /// Sets my status message (`v1/me/publishnote`, as the web client does); an empty message
    /// clears it. `expiry` is when it goes away by itself; `None` keeps it until changed.
    pub fn set_status_note(&mut self, message: &str, expiry: Option<chrono::DateTime<chrono::Utc>>) -> Result<(), String> {
        let token = self.auth.token(SCOPE_PRESENCE)?;
        let expiry = expiry.map(|e| e.format("%Y-%m-%dT%H:%M:%S.000Z").to_string()).unwrap_or_else(|| "9999-12-31T23:59:59.999Z".into());
        // Empty means none: an empty paragraph would be a (blank) message of its own.
        let text = message.trim();
        let message = if text.is_empty() { String::new() } else { crate::html::from_plain_text(text) };
        let body = json!({ "message": message, "expiry": expiry });
        let resp = self
            .http
            .put(format!("{}/v1/me/publishnote", self.presence))
            .bearer_auth(token)
            .json(&body)
            .send()
            .map_err(crate::api::unreachable("Teams"))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            let status = resp.status();
            Err(format!("Setting your status message failed ({status}). {}", resp.text().unwrap_or_default().chars().take(160).collect::<String>()))
        }
    }

    /// Registers (or refreshes) this app as one of my presence endpoints, the way each Teams
    /// client does; the service combines all of them into my status.
    pub fn register_endpoint(&mut self, endpoint_id: &str, active: bool) -> Result<(), String> {
        let token = self.auth.token(SCOPE_PRESENCE)?;
        let state = if active { "Available" } else { "Away" };
        let resp = self
            .http
            .put(format!("{}/v1/me/endpoints/", self.presence))
            .bearer_auth(token)
            .header("x-ms-client-user-agent", CLIENT_AGENT)
            .header("x-ms-endpoint-id", endpoint_id)
            .json(&json!({
                "id": endpoint_id,
                "availability": state,
                "activity": state,
                "deviceType": "Desktop",
            }))
            .send()
            .map_err(crate::api::unreachable("Teams"))?;
        presence_result("Registering for presence", resp)
    }

    /// Tells the presence service whether I'm active at this endpoint (the web client does
    /// this about once a minute; idle endpoints turn me Away).
    pub fn report_activity(&mut self, endpoint_id: &str, active: bool) -> Result<(), String> {
        let token = self.auth.token(SCOPE_PRESENCE)?;
        let resp = self
            .http
            .post(format!("{}/v1/me/reportmyactivity/", self.presence))
            .bearer_auth(token)
            .header("x-ms-client-user-agent", CLIENT_AGENT)
            .header("x-ms-endpoint-id", endpoint_id)
            .json(&json!({ "endpointId": endpoint_id, "isActive": active }))
            .send()
            .map_err(crate::api::unreachable("Teams"))?;
        presence_result("Reporting activity", resp)
    }

    /// Removes this app's endpoint (signing out), so it stops counting toward my status.
    pub fn unregister_endpoint(&mut self, endpoint_id: &str) -> Result<(), String> {
        let token = self.auth.token(SCOPE_PRESENCE)?;
        let resp = self
            .http
            .delete(format!("{}/v1/me/endpoints/{}", self.presence, crate::api::urlencode(endpoint_id)))
            .bearer_auth(token)
            .header("x-ms-client-user-agent", CLIENT_AGENT)
            .header("x-ms-endpoint-id", endpoint_id)
            .send()
            .map_err(crate::api::unreachable("Teams"))?;
        presence_result("Unregistering", resp)
    }

    /// Where a person's profile picture is, and the token to fetch it with.
    pub fn picture_request(&mut self, mri: &str) -> Result<(String, String), String> {
        let token = self.auth.token(crate::api::SCOPE_SKYPE)?;
        let url = format!(
            "{GATEWAY}/mt/part/{}/beta/users/{}/profilepicturev2?size=HR96x96",
            self.mt_partition(),
            crate::api::urlencode(mri)
        );
        Ok((url, token))
    }

    /// A person's profile picture bytes; `Ok(None)` when they have none.
    pub fn profile_picture(&mut self, mri: &str) -> Result<Option<Vec<u8>>, String> {
        let (url, token) = self.picture_request(mri)?;
        fetch_picture(&self.http, &token, &url)
    }
}

/// Downloads a profile picture; `Ok(None)` when the person has no photo (the service then
/// answers with drawn initials as SVG, or nothing).
pub fn fetch_picture(http: &reqwest::blocking::Client, token: &str, url: &str) -> Result<Option<Vec<u8>>, String> {
    let resp = http
        .get(url)
        .bearer_auth(token)
        .send()
        .map_err(crate::api::unreachable("Teams"))?;
    let status = resp.status();
    if status.as_u16() == 404 || status.as_u16() == 204 {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(format!("Loading a picture failed ({status})"));
    }
    let is_image = resp
        .headers()
        .get("content-type")
        .and_then(|c| c.to_str().ok())
        .is_some_and(|c| c.starts_with("image/") && !c.contains("svg"));
    let bytes = resp.bytes().map_err(|e| e.to_string())?.to_vec();
    Ok(if is_image { Some(bytes) } else { None })
}

fn presence_result(what: &str, resp: reqwest::blocking::Response) -> Result<(), String> {
    let status = resp.status();
    if status.is_success() {
        Ok(())
    } else {
        let body: String = resp.text().unwrap_or_default().chars().take(160).collect();
        Err(format!("{what} failed ({status}). {body}"))
    }
}

/// A person found by the new-chat search.
#[derive(Debug, Clone, PartialEq)]
pub struct Person {
    pub mri: String,
    pub name: String,
    pub email: String,
    pub job_title: String,
}

impl Teams {
    /// People in my organisation whose name or address starts with `query`, through Microsoft
    /// Graph's directory search (people I work with most come first via /me/people).
    pub fn search_people(&mut self, query: &str) -> Result<Vec<Person>, String> {
        let token = self.auth.token(crate::attachments::SCOPE_GRAPH)?;
        let q = query.replace('"', "");
        let mut found: Vec<Person> = Vec::new();
        let mut add = |v: &Value, id_key: &str, mail_key: &str| {
            for p in v["value"].as_array().into_iter().flatten() {
                let Some(id) = p[id_key].as_str() else { continue };
                let mri = format!("8:orgid:{id}");
                if found.iter().any(|f| f.mri == mri) {
                    continue;
                }
                let email = p[mail_key]
                    .as_str()
                    .or_else(|| p["scoredEmailAddresses"][0]["address"].as_str())
                    .or_else(|| p["userPrincipalName"].as_str())
                    .unwrap_or("")
                    .to_string();
                found.push(Person {
                    mri,
                    name: p["displayName"].as_str().unwrap_or("").to_string(),
                    email,
                    job_title: p["jobTitle"].as_str().unwrap_or("").to_string(),
                });
            }
        };
        // Relevant people first; it only knows people I've worked with.
        if let Ok(resp) = self
            .http
            .get("https://graph.microsoft.com/v1.0/me/people")
            .query(&[("$search", format!("\"{q}\"")), ("$top", "10".into()), ("$filter", "personType/subclass eq 'OrganizationUser'".into())])
            .bearer_auth(&token)
            .send()
        {
            if resp.status().is_success() {
                if let Ok(v) = resp.json::<Value>() {
                    add(&v, "id", "userPrincipalName");
                }
            }
        }
        // Then everyone in the directory.
        let resp = self
            .http
            .get("https://graph.microsoft.com/v1.0/users")
            .query(&[
                ("$search", format!("\"displayName:{q}\" OR \"mail:{q}\"")),
                ("$select", "id,displayName,mail,userPrincipalName,jobTitle".into()),
                ("$top", "15".into()),
            ])
            .header("ConsistencyLevel", "eventual")
            .bearer_auth(&token)
            .send()
            .map_err(crate::api::unreachable("Microsoft"))?;
        let status = resp.status();
        let text = resp.text().unwrap_or_default();
        if status.is_success() {
            let v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            add(&v, "id", "mail");
        } else if found.is_empty() {
            return Err(format!("Searching people failed ({status}). {}", text.chars().take(160).collect::<String>()));
        }
        found.retain(|p| p.mri != self.my_mri && !p.name.is_empty());
        Ok(found)
    }

    /// The one-to-one chat with `mri`: the existing one if there is one, otherwise a new,
    /// empty one (nobody is told until a message is sent). Same request as the web client:
    /// a "unique roster" chat thread with the two of us.
    pub fn open_chat_with(&mut self, mri: &str) -> Result<String, String> {
        let body = json!({
            "members": [
                { "id": self.my_mri, "role": "Admin" },
                { "id": mri, "role": "Admin" },
            ],
            "properties": {
                "threadType": "chat",
                "chatFilesIndexId": "2",
                "fixedRoster": "true",
                "uniquerosterthread": "true",
            },
        });
        let (status, location, text) = self.chat_post_with_location("/v1/threads", &body)?;
        if !status.is_success() {
            return Err(format!("Starting the chat failed ({status}). {}", text.chars().take(160).collect::<String>()));
        }
        // The new thread's address comes back in Location: .../v1/threads/{id}
        let from_location = location.rsplit('/').next().map(|s| s.split('?').next().unwrap_or(s).to_string());
        let from_body = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v["threadResource"]["id"].as_str().or(v["id"].as_str()).map(str::to_string));
        from_location
            .filter(|s| s.starts_with("19:"))
            .or(from_body)
            .map(|id| crate::api::urldecode(&id))
            .ok_or_else(|| "Teams didn't say which chat it opened.".into())
    }
}
