//! Teams' call service exchanges SDP in its own dialect, `application/sdp-ngc-1.0`. The web
//! client converts both ways around a standard WebRTC stack; these are the same rewrites,
//! learned by diffing what the service sent with what the browser applied.

/// The media stream id of our screen-share track, which marks its section.
pub const SHARE_STREAM: &str = "notteams-share";
/// Teams' label for a screen-share section.
pub const SHARE_LABEL: &str = "applicationsharing-video";

/// Standard WebRTC offer → the dialect the call service expects.
pub fn to_ngc(sdp: &str) -> String {
    let lines: Vec<&str> = sdp.lines().map(|l| l.trim_end_matches('\r')).filter(|l| !l.is_empty()).collect();
    // The m-line and c-line carry the best candidate's address, as the web client sends: the
    // relay when there is one, then the public address, then a host address (container
    // bridges like Docker's 172.17.0.1 last, nobody else can reach them).
    let rank = |typ: &str, ip: &str| match typ {
        "relay" => 0,
        "srflx" => 1,
        _ if ip.starts_with("172.17.") || ip.starts_with("172.18.") => 3,
        _ => 2,
    };
    let host = lines
        .iter()
        .filter_map(|l| {
            let f: Vec<&str> = l.strip_prefix("a=candidate:")?.split_whitespace().collect();
            (f.len() >= 8 && f[2].eq_ignore_ascii_case("udp") && !f[4].contains(':')).then(|| (rank(f[7], f[4]), f[4].to_string(), f[5].to_string()))
        })
        .min_by_key(|c| c.0)
        .map(|(_, ip, port)| (ip, port));
    // Teams clients expect the DTLS fingerprint in every media section, as the web client
    // writes it, not once for the whole session.
    let fingerprint = lines.iter().find(|l| l.starts_with("a=fingerprint:")).map(|l| l.to_string());
    let mut in_media = false;
    // Each media section's own kind and SSRC, for the lines Teams wants after its `a=mid`.
    let mut section_info = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if let Some(m) = l.strip_prefix("m=") {
            let kind = m.split_whitespace().next().unwrap_or("").to_string();
            let section = lines[i + 1..].iter().take_while(|x| !x.starts_with("m="));
            let ssrc = section.clone().find_map(|x| x.strip_prefix("a=ssrc:")?.split_whitespace().next().map(str::to_string));
            // Our screen-share track is the one in the stream named SHARE_STREAM.
            let share = section.clone().any(|x| x.starts_with("a=msid:") && x.contains(SHARE_STREAM));
            section_info.push((kind, ssrc, share));
        }
    }

    let mut out = Vec::new();
    let mut section = 0usize;
    let mut data = false;
    for line in &lines {
        if line.starts_with("a=candidate:") && line.contains(" tcp ") {
            continue;
        }
        // With rtcp-mux only component 1 is used; the web client sends no others.
        if line.starts_with("a=candidate:") && line.split_whitespace().nth(1) == Some("2") {
            continue;
        }
        if *line == "a=end-of-candidates" || (line.starts_with("a=fingerprint:") && !in_media) {
            continue;
        }
        if line.starts_with("a=fingerprint:") {
            continue;
        }
        // The data channel's section, written the way the web client sends it.
        if let Some(rest) = line.strip_prefix("m=application ") {
            section += 1;
            data = true;
            let port = host.as_ref().map(|(_, p)| p.as_str()).unwrap_or_else(|| rest.split(' ').next().unwrap_or("9"));
            out.push(format!("m=x-data {port} RTP/SAVP 127 126"));
            continue;
        }
        if line.starts_with("m=") {
            data = false;
        }
        if data {
            if line.starts_with("c=") {
                out.push(match &host {
                    Some((ip, _)) => format!("c=IN IP4 {ip}"),
                    None => line.to_string(),
                });
                let ssrc = rand::random::<u32>() >> 1;
                out.extend([
                    "a=x-data-protocol:sctp".to_string(),
                    format!("a=x-ssrc-range:{ssrc}-{ssrc}"),
                    "a=rtpmap:127 x-data/90000".into(),
                    "a=rtpmap:126 rtx/90000".into(),
                    "a=fmtp:126 apt=127".into(),
                ]);
                continue;
            }
            out.push(line.to_string());
            if line.starts_with("a=setup:") {
                out.extend(fingerprint.iter().cloned());
            }
            if line.starts_with("a=mid:") {
                out.extend(["a=sendrecv", "a=rtcp-mux", "a=label:data"].map(str::to_string));
            }
            continue;
        }
        if line.starts_with("m=") {
            section += 1;
            let mut f: Vec<String> = line.split_whitespace().map(str::to_string).collect();
            if f.len() > 2 {
                if let Some((_, port)) = &host {
                    f[1] = port.clone();
                }
                f[2] = "RTP/SAVP".into();
            }
            out.push(f.join(" "));
            continue;
        }
        if line.starts_with("m=") {
            in_media = true;
        }
        if line.starts_with("a=setup:") {
            out.push(line.to_string());
            out.extend(fingerprint.iter().cloned());
            continue;
        }
        if line.starts_with("c=IN IP4") {
            if let Some((ip, port)) = &host {
                out.push(format!("c=IN IP4 {ip}"));
                out.push(format!("a=rtcp:{port}"));
                continue;
            }
        }
        if let Some(rest) = line.strip_prefix("a=extmap:") {
            out.push(format!("a=extmap:{}", rest.replace('/', "\\")));
            continue;
        }
        out.push(line.to_string());
        if line.starts_with("s=") {
            out.push("b=CT:4000".into());
        }
        if line.starts_with("a=mid:") {
            let (kind, ssrc, share) = section.checked_sub(1).and_then(|i| section_info.get(i)).cloned().unwrap_or_default();
            let video = kind == "video";
            out.push(format!("a=label:{}", if share { SHARE_LABEL } else if video { "main-video" } else { "main-audio" }));
            if let Some(s) = ssrc {
                out.push(format!("a=x-ssrc-range:{s}-{s}"));
            }
            out.push(if video { "a=x-signaling-fb:* x-message app send:src recv:src,vc" } else { "a=x-signaling-fb:* x-message app recv:dsh" }.into());
        }
    }
    out.join("\r\n") + "\r\n"
}

/// The call service's answer → standard SDP a WebRTC stack accepts.
pub fn from_ngc(sdp: &str) -> String {
    let mut out = Vec::new();
    // A section the media processor declined (port 0) comes without its `a=mid`, which a
    // WebRTC stack refuses; ours are numbered by position, so it's restored from there.
    let mut section: Option<usize> = None;
    let mut has_mid = true;
    // In the data channel's section only the transport lines carry over; a WebRTC stack
    // wants its SCTP port, which Teams leaves out.
    let mut data = false;
    let finish_data = |out: &mut Vec<String>, data: bool| {
        if data && !out.iter().rev().take_while(|l| !l.starts_with("m=")).any(|l| l.starts_with("a=sctp-port:")) {
            out.push("a=sctp-port:5000".into());
        }
    };
    let lines: Vec<&str> = sdp.lines().map(|l| l.trim_end_matches('\r')).filter(|l| !l.is_empty()).collect();
    for line in lines {
        if line.starts_with("m=") {
            if let (Some(n), false) = (section, has_mid) {
                out.push(format!("a=mid:{n}"));
            }
            finish_data(&mut out, data);
            section = Some(section.map_or(0, |n| n + 1));
            has_mid = false;
            data = line.starts_with("m=x-data ");
            if data {
                let port = line.split(' ').nth(1).unwrap_or("9");
                out.push(format!("m=application {port} UDP/DTLS/SCTP webrtc-datachannel"));
            } else {
                out.push(line.replacen(" RTP/SAVP ", " UDP/TLS/RTP/SAVPF ", 1));
            }
            continue;
        }
        if data {
            const KEEP: [&str; 8] = ["c=", "a=setup:", "a=mid:", "a=ice-", "a=fingerprint:", "a=candidate:", "a=sctp-port:", "a=max-message-size:"];
            if !KEEP.iter().any(|k| line.starts_with(k)) {
                continue;
            }
        }
        if line.starts_with("a=mid:") {
            has_mid = true;
        }
        if let Some(c) = line.strip_prefix("a=candidate:") {
            // `tcp-pass`/`tcp-act` candidates and the `MTURNID` suffix are Teams-only; the
            // UDP candidates are all a call needs.
            let f: Vec<&str> = c.split_whitespace().collect();
            if f.len() < 8 || !f[2].eq_ignore_ascii_case("udp") {
                continue;
            }
            let end = f.iter().position(|t| *t == "MTURNID").unwrap_or(f.len());
            out.push(format!("a=candidate:{}", f[..end].join(" ")));
            continue;
        }
        // Teams answers the RID extensions too, then tags packets with stream ids we never
        // declared; a WebRTC stack takes those for unknown simulcast layers and drops them.
        if line.starts_with("a=extmap:") && line.contains("rtp-stream-id") {
            continue;
        }
        if let Some(rest) = line.strip_prefix("a=extmap:") {
            out.push(format!("a=extmap:{}", rest.replace('\\', "/")));
            continue;
        }
        if let Some(rest) = line.strip_prefix("a=rtpmap:") {
            let (pt, codec) = rest.split_once(' ').unwrap_or((rest, ""));
            let codec = match codec.to_ascii_uppercase().as_str() {
                "RED/8000" => "red/48000/2".to_string(),
                c if c.starts_with("OPUS/") => codec.to_ascii_lowercase(),
                _ => codec.to_string(),
            };
            out.push(format!("a=rtpmap:{pt} {codec}"));
            continue;
        }
        // The range holds the SSRCs Teams will send on this m-line. The media processor sends a
        // section's stream on the range's first SSRC (its retransmissions 50 above, named by an
        // `a=ssrc-group:FID` when RTX was negotiated), whichever source it forwards there. The
        // WebRTC stack only routes SSRCs the SDP declares (or simulcast rids), so declare it.
        if let Some(range) = line.strip_prefix("a=x-ssrc-range:") {
            if let Some((a, _)) = range.split_once('-') {
                out.push(format!("a=ssrc:{a} cname:teams{a}"));
                out.push(format!("a=ssrc:{a} msid:teams-{a} teams-{a}"));
            }
            continue;
        }
        if line.starts_with("a=x-") {
            continue;
        }
        out.push(line.to_string());
    }
    if let (Some(n), false) = (section, has_mid) {
        out.push(format!("a=mid:{n}"));
    }
    finish_data(&mut out, data);
    out.join("\r\n") + "\r\n"
}

/// Our id on the data channel: the `x-source-streamid` of the answer's `x-data` section.
#[allow(dead_code)]
pub fn data_stream_id(ngc: &str) -> Option<u32> {
    let mut in_data = false;
    for line in ngc.lines().map(|l| l.trim_end_matches('\r')) {
        if line.starts_with("m=") {
            in_data = line.starts_with("m=x-data");
        } else if in_data {
            if let Some(v) = line.strip_prefix("a=x-source-streamid:") {
                return v.trim().parse().ok();
            }
        }
    }
    None
}

/// An offer from Teams' media processor (incoming calls) → what a WebRTC stack accepts,
/// plus what's needed to shape our answer: it offers SDES keys alongside the DTLS
/// fingerprint, RTP/AVP, two-component candidates and a Teams-only `x-data` section.
pub struct OfferConversion {
    pub sdp: String,
    /// The offer's transport for audio, mirrored in our answer.
    pub proto: String,
    /// Sections we can't take (`m=x-data`...), to be declined in the answer with port 0.
    pub declined: Vec<(String, String)>,
}

pub fn offer_from_mp(sdp: &str) -> OfferConversion {
    offer_to_std(sdp, false)
}

/// An offer from the other side of a one-to-one call (renegotiation: the other person turned
/// video on...) → standard SDP: its audio and video sections kept, Teams-only ones declined.
pub fn peer_offer(sdp: &str) -> OfferConversion {
    offer_to_std(sdp, true)
}

fn offer_to_std(sdp: &str, keep_video: bool) -> OfferConversion {
    let lines: Vec<&str> = sdp.lines().map(|l| l.trim_end_matches('\r')).filter(|l| !l.is_empty()).collect();
    let first_m = lines.iter().position(|l| l.starts_with("m=")).unwrap_or(lines.len());
    let mut sections: Vec<Vec<&str>> = Vec::new();
    for l in &lines[first_m..] {
        if l.starts_with("m=") {
            sections.push(Vec::new());
        }
        if let Some(sec) = sections.last_mut() {
            sec.push(l);
        }
    }
    let mid_of = |sec: &Vec<&str>| sec.iter().find_map(|l| l.strip_prefix("a=mid:")).unwrap_or("").to_string();
    let mut kept = Vec::new();
    let mut declined = Vec::new();
    let mut proto = "RTP/AVP".to_string();
    for sec in &sections {
        let m: Vec<&str> = sec[0].split_whitespace().collect();
        let audio = m.first() == Some(&"m=audio");
        if (audio && kept.is_empty()) || (keep_video && m.first() == Some(&"m=video")) {
            if audio {
                proto = m.get(2).unwrap_or(&"RTP/AVP").to_string();
            }
            kept.push(sec.clone());
        } else {
            let pt = m.get(3).unwrap_or(&"0");
            declined.push((format!("{} 0 {} {pt}", m[0], m.get(2).unwrap_or(&"RTP/AVP")), mid_of(sec)));
        }
    }
    let kept_mids: Vec<String> = kept.iter().map(mid_of).collect();

    let mut out = Vec::new();
    for l in &lines[..first_m] {
        if l.starts_with("a=group:BUNDLE") {
            out.push(format!("a=group:BUNDLE {}", kept_mids.join(" ")));
        } else if !l.starts_with("a=x-") {
            out.push(l.to_string());
        }
    }
    for sec in &kept {
        let has_setup = sec.iter().any(|l| l.starts_with("a=setup:"));
        for (n, l) in sec.iter().enumerate() {
            if n == 0 {
                let mut f: Vec<String> = l.split_whitespace().map(str::to_string).collect();
                if f.len() > 2 {
                    f[2] = "UDP/TLS/RTP/SAVPF".into();
                }
                out.push(f.join(" "));
                continue;
            }
            if l.starts_with("a=crypto:") || l.starts_with("a=rtcp:") {
                continue;
            }
            if let Some(c) = l.strip_prefix("a=candidate:") {
                let f: Vec<&str> = c.split_whitespace().collect();
                if f.len() < 8 || f[1] != "1" || !f[2].eq_ignore_ascii_case("udp") {
                    continue;
                }
                let end = f.iter().position(|t| *t == "MTURNID").unwrap_or(f.len());
                out.push(format!("a=candidate:{}", f[..end].join(" ")));
                continue;
            }
            if let Some(rest) = l.strip_prefix("a=extmap:") {
                out.push(format!("a=extmap:{}", rest.replace('\\', "/")));
                continue;
            }
            // An offering client sends from the start of its block (the web client declares
            // that SSRC, with no mid extension to route by otherwise).
            if let Some(range) = l.strip_prefix("a=x-ssrc-range:") {
                if let Some((a, _)) = range.split_once('-') {
                    out.push(format!("a=ssrc:{a} cname:teams{a}"));
                    out.push(format!("a=ssrc:{a} msid:teams-{a} teams-{a}"));
                }
                continue;
            }
            if l.starts_with("a=extmap:") && l.contains("rtp-stream-id") {
                continue;
            }
            if l.starts_with("a=x-") || l.starts_with("a=rtcp-fb:* x-") {
                continue;
            }
            out.push(l.to_string());
        }
        if !has_setup {
            out.push("a=setup:actpass".into());
        }
        let directions = ["a=sendrecv", "a=sendonly", "a=recvonly", "a=inactive"];
        if !sec.iter().any(|l| directions.contains(l)) {
            out.push("a=sendrecv".into());
        }
    }
    OfferConversion {
        sdp: out.join("\r\n") + "\r\n",
        proto,
        declined,
    }
}

/// Our standard answer → Teams' dialect, with the sections we couldn't take declined.
pub fn answer_to_ngc(std_answer: &str, conv: &OfferConversion) -> String {
    let mut ngc = to_ngc(std_answer);
    ngc = ngc
        .lines()
        .map(|l| {
            if l.starts_with("m=audio") {
                l.replacen(" RTP/SAVP ", &format!(" {} ", conv.proto), 1)
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\r\n")
        + "\r\n";
    for (mline, mid) in &conv.declined {
        ngc.push_str(&format!("{mline}\r\na=mid:{mid}\r\n"));
    }
    ngc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answer_becomes_standard() {
        let ngc = "v=0\r\nm=audio 3478 RTP/SAVP 111\r\na=rtpmap:111 OPUS/48000/2\r\n\
                   a=candidate:1 1 UDP 54001663 52.115.132.191 3478 typ relay raddr 10.0.16.253 rport 3478 MTURNID 11\r\n\
                   a=candidate:3 1 tcp-pass 18087935 52.115.132.191 3478 typ relay raddr 10.0.16.253 rport 3478\r\n\
                   a=extmap:2 http:\\\\www.webrtc.org\\experiments\\rtp-hdrext\\abs-send-time\r\na=x-ssrc-range:1000-1000\r\n";
        let std = from_ngc(ngc);
        assert!(std.contains("m=audio 3478 UDP/TLS/RTP/SAVPF 111"));
        assert!(std.contains("a=rtpmap:111 opus/48000/2"));
        assert!(std.contains("rport 3478\r\n"));
        assert!(!std.contains("tcp-pass") && !std.contains("MTURNID") && !std.contains("x-ssrc"));
        assert!(std.contains("a=ssrc:1000 cname:"));
        assert!(std.contains("a=extmap:2 http://www.webrtc.org/experiments/rtp-hdrext/abs-send-time"));
    }

    #[test]
    fn media_processor_offer_becomes_standard() {
        let mp = "v=0\r\ns=session\r\na=group:BUNDLE 1 2\r\nm=audio 52812 RTP/AVP 0 8\r\na=rtcp:52813\r\n\
                  a=candidate:1 1 UDP 2130706431 52.115.113.185 52812 typ srflx raddr 10.0.17.9 rport 52812\r\n\
                  a=candidate:1 2 UDP 2130705918 52.115.113.185 52813 typ srflx raddr 10.0.17.9 rport 52813\r\n\
                  a=candidate:2 1 tcp-act 2121006078 52.115.113.185 49152 typ srflx raddr 10.0.17.9 rport 49152\r\n\
                  a=mid:1\r\na=crypto:1 AES_CM_128_HMAC_SHA1_80 inline:abc\r\na=x-ssrc-range:7-7\r\n\
                  m=x-data 62530 RTP/AVP 127 126\r\na=mid:2\r\n";
        let c = offer_from_mp(mp);
        assert!(c.sdp.contains("a=group:BUNDLE 1\r\n"));
        assert!(c.sdp.contains("m=audio 52812 UDP/TLS/RTP/SAVPF 0 8"));
        assert!(c.sdp.contains("a=setup:actpass") && c.sdp.contains("a=ssrc:7 cname:"));
        assert!(!c.sdp.contains("crypto") && !c.sdp.contains("x-data") && !c.sdp.contains(" 2 UDP") && !c.sdp.contains("tcp-act"));
        assert_eq!(c.declined, vec![("m=x-data 0 RTP/AVP 127".to_string(), "2".to_string())]);
        let answer = answer_to_ngc("v=0\r\ns=-\r\nm=audio 9 UDP/TLS/RTP/SAVPF 0\r\na=mid:1\r\n", &c);
        assert!(answer.contains("m=audio 9 RTP/AVP 0"));
        assert!(answer.ends_with("m=x-data 0 RTP/AVP 127\r\na=mid:2\r\n"));
    }

    #[test]
    fn video_sections_get_their_own_label_and_ssrc() {
        let offer = "v=0\r\ns=-\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\na=mid:0\r\na=ssrc:1 cname:x\r\n\
                     m=video 9 UDP/TLS/RTP/SAVPF 107\r\na=mid:1\r\na=ssrc:2 cname:x\r\n";
        let ngc = to_ngc(offer);
        assert!(ngc.contains("a=mid:0\r\na=label:main-audio\r\na=x-ssrc-range:1-1"));
        assert!(ngc.contains("a=mid:1\r\na=label:main-video\r\na=x-ssrc-range:2-2\r\na=x-signaling-fb:* x-message app send:src recv:src,vc"));
    }

    #[test]
    fn video_slot_and_ssrc_block() {
        let ngc = "v=0\r\nm=audio 1 RTP/SAVP 111\r\na=x-source-streamid:2680\r\nm=video 1 RTP/SAVP 107\r\na=x-source-streamid:2681\r\na=x-ssrc-range:111713-111812\r\n";
        assert_eq!(video_stream_id(ngc), Some(2681));
        assert_eq!(receive_slot(ngc), None, "no mid in this snippet");
        let two = "m=video 1 RTP/SAVP 107\r\na=x-source-streamid:2681\r\na=mid:1\r\nm=video 1 RTP/SAVP 107\r\na=x-source-streamid:2682\r\na=mid:2\r\n";
        assert_eq!(receive_slot(two), Some(("2".to_string(), 2682)));
        let std = from_ngc(ngc);
        assert!(std.contains("a=ssrc:111713 cname:"), "a block's stream arrives on its first SSRC");
        assert!(!std.contains("a=ssrc:111714"));
    }

    #[test]
    fn rid_extensions_are_dropped() {
        let ngc = "v=0\r\nm=video 1 RTP/SAVP 107\r\na=mid:2\r\na=extmap:1 urn:ietf:params:rtp-hdrext:sdes:mid\r\na=extmap:2 urn:ietf:params:rtp-hdrext:sdes:rtp-stream-id\r\na=extmap:3 urn:ietf:params:rtp-hdrext:sdes:repaired-rtp-stream-id\r\n";
        let std = from_ngc(ngc);
        assert!(std.contains("sdes:mid") && !std.contains("rtp-stream-id"));
    }

    #[test]
    fn declined_section_gets_its_mid_back() {
        let ngc = "v=0\r\nm=audio 1 RTP/SAVP 111\r\na=mid:0\r\nm=video 0 RTP/SAVP 107\r\n";
        assert!(from_ngc(ngc).ends_with("m=video 0 UDP/TLS/RTP/SAVPF 107\r\na=mid:1\r\n"));
    }

    #[test]
    fn offer_becomes_ngc() {
        let offer = "v=0\r\ns=-\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\nc=IN IP4 0.0.0.0\r\na=mid:0\r\n\
                     a=ssrc:42 cname:x\r\na=candidate:1 1 udp 2130706431 192.168.1.5 50000 typ host\r\n";
        let ngc = to_ngc(offer);
        assert!(ngc.contains("b=CT:4000"));
        assert!(ngc.contains("m=audio 50000 RTP/SAVP 111"));
        assert!(ngc.contains("c=IN IP4 192.168.1.5"));
        assert!(ngc.contains("a=x-ssrc-range:42-42"));
    }
}

/// One section of a Teams answer that others' video can be sent into: its mid, its
/// `a=x-source-streamid` (the `streamMsid` of a source request) and the first SSRC of its
/// range, which the media processor sends that video on.
#[derive(Debug, Clone, PartialEq)]
pub struct Slot {
    pub mid: String,
    pub stream: u32,
    pub ssrc: Option<u32>,
}

/// The receive slots of a Teams answer: every accepted video section after the first (our own
/// camera), or the only one.
pub fn receive_slots(ngc: &str) -> Vec<Slot> {
    let mut sections: Vec<Slot> = video_sections(ngc).into_iter().filter(|(_, share)| !share).map(|(s, _)| s).collect();
    if sections.len() > 1 {
        sections.remove(0);
    }
    sections
}

/// The answer's screen-share section, where others' shared screens are sent.
pub fn share_slot(ngc: &str) -> Option<Slot> {
    video_sections(ngc).into_iter().find(|(_, share)| *share).map(|(s, _)| s)
}

/// Every accepted video section, with whether it's the screen-share one.
fn video_sections(ngc: &str) -> Vec<(Slot, bool)> {
    let mut sections: Vec<(Slot, bool)> = Vec::new();
    let mut cur: Option<(bool, Option<String>, Option<u32>, Option<u32>, bool)> = None;
    let flush = |cur: &mut Option<(bool, Option<String>, Option<u32>, Option<u32>, bool)>, out: &mut Vec<(Slot, bool)>| {
        if let Some((true, Some(mid), Some(stream), ssrc, share)) = cur.take() {
            out.push((Slot { mid, stream, ssrc }, share));
        }
    };
    for line in ngc.lines().map(|l| l.trim_end_matches('\r')) {
        if let Some(m) = line.strip_prefix("m=") {
            flush(&mut cur, &mut sections);
            let accepted = m.starts_with("video ") && m.split(' ').nth(1) != Some("0");
            cur = Some((accepted, None, None, None, false));
        } else if let Some((_, mid, stream, ssrc, share)) = cur.as_mut() {
            if line == format!("a=label:{SHARE_LABEL}") {
                *share = true;
            }
            if let Some(v) = line.strip_prefix("a=x-source-streamid:") {
                *stream = v.trim().parse().ok();
            } else if let Some(v) = line.strip_prefix("a=mid:") {
                *mid = Some(v.trim().to_string());
            } else if let Some(v) = line.strip_prefix("a=x-ssrc-range:") {
                *ssrc = v.split('-').next().and_then(|a| a.trim().parse().ok());
            }
        }
    }
    flush(&mut cur, &mut sections);
    sections
}

/// The first receive slot as (mid, stream id).
#[allow(dead_code)]
pub fn receive_slot(ngc: &str) -> Option<(String, u32)> {
    receive_slots(ngc).into_iter().next().map(|s| (s.mid, s.stream))
}

/// One line per section for the call log: media, mid, direction, label, stream id and SSRC
/// range, e.g. `video mid 2 sendrecv main-video stream 629 ssrc 600-699`.
pub fn outline(ngc: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for line in ngc.lines().map(|l| l.trim_end_matches('\r')) {
        if let Some(m) = line.strip_prefix("m=") {
            out.push(m.split(' ').next().unwrap_or("").to_string());
            continue;
        }
        let Some(cur) = out.last_mut() else { continue };
        let a = line.strip_prefix("a=").unwrap_or("");
        let add = match a.split_once(':') {
            Some(("mid", v)) => format!("mid {v}"),
            Some(("label", v)) => v.to_string(),
            Some(("x-source-streamid", v)) => format!("stream {v}"),
            Some(("x-ssrc-range", v)) => format!("ssrc {v}"),
            None if matches!(a, "sendrecv" | "sendonly" | "recvonly" | "inactive") => a.to_string(),
            _ => continue,
        };
        cur.push(' ');
        cur.push_str(&add);
    }
    out.join("; ")
}

/// The `a=x-source-streamid` of the first video section of a Teams answer (our camera).
pub fn video_stream_id(ngc: &str) -> Option<u32> {
    let mut in_video = false;
    for line in ngc.lines().map(|l| l.trim_end_matches('\r')) {
        if line.starts_with("m=") {
            in_video = line.starts_with("m=video");
        } else if in_video {
            if let Some(id) = line.strip_prefix("a=x-source-streamid:") {
                return id.trim().parse().ok();
            }
        }
    }
    None
}
