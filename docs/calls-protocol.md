# Teams calls: how the web client does it

Observed on 2026-11-18 by recording the Teams web client (teams.cloud.microsoft) while it
placed Teams' own test call ("Make a test call" in Settings → Devices). The call connected.
Values below are shapes, not secrets: tokens were never kept.

## The test call partner

- "Make a test call" talks to the **Echo** bot, MRI `28:cf28171e-fcfd-47e4-a1d6-79460b0b3ca0`.
  It answers, plays a prompt, records a few seconds and plays them back: a full audio round
  trip with nobody on the other end.
- The web client first opens the one-to-one chat with it, with the same create-or-reuse thread
  request NotTeams already uses for new chats (`POST {chatsvc}/v1/threads`, members me +
  `28:cf28171e-…`, `uniquerosterthread`).
- The call itself is marked `"scenario": "UserInitiatedTestCall"`.

## Signalling

All call requests go through the **flightproxy** with an AAD Bearer token and these headers:
`MS-Teams-Partition` (e.g. `emea02`), `MS-Teams-Region` (`emea`), `MS-Teams-Ring` (`general`),
`X-Microsoft-Skype-Client`, `X-Microsoft-Skype-Message-ID` (new UUID per request) and
`X-Microsoft-Skype-Chain-ID` (one UUID per call), `X-MS-Migration: True`.

1. **Start**: `POST https://api-{region}.flightproxy.teams.microsoft.com/api/v2/epconv` → 201.
   Body:
   - `conversationRequest`: `roster` (`type: Delta`, `rosterUpdate` callback URL), `links`
     (callback URLs: `conversationEnd`, `conversationUpdate`, `localParticipantUpdate`,
     `addParticipantSuccess/Failure`, `addModalitySuccess/Failure`, `confirmUnmute`,
     `receiveMessage`), `properties`, `scenario`.
   - `participants.from`: my MRI, display name, `endpointId` (one per app install),
     `participantId` (new per call), `languageId`.
   - `callInvitation`: `callModalities` (`["Audio","Video"]`), `mediaContent` with
     `contentType: "application/sdp-ngc-1.0"`, `blob` = my WebRTC **SDP offer**,
     `mediaDescriptions` (mid, direction, label), `mediaLegId` (new 32-hex id),
     `clientLocation`, `applyChannelParameters`.
   - `endpointCapabilities`, `clientEndpointCapabilities`, `endpointState`, `endpointMetadata`.
   - Response: `conversationController` URL (on `conv-*.conv.skype.com` via the flightproxy),
     `activeModalities.call.links` for the call's own actions.
2. **Callbacks** arrive over **Trouter** (Teams' push channel, a socket.io-style WebSocket):
   the callback URLs in step 1 are paths on my Trouter endpoint
   (`…/callAgent/{agentId}/{random}/conversation/conversationUpdate/` etc.). The **SDP
   answer** comes in `…/call/acceptance/`; roster and state changes in `rosterUpdate`,
   `conversationUpdate`. Trouter delivers them as `3:::{"id":…,"method":"POST","url":…,
   "headers":…,"body":…}` frames and expects `3:::{"id":…,"status":200,…}` back.
3. **During the call**: `PUT …/conv/{id}/updateEndpoint`, `PUT …/updateEndpointMetadata`,
   `POST …/cc/v1/mcProxy/…/applyChannelParameters` (video caps), and a long-poll
   `GET …/broker.skype.com/api/v1/subscribe/{id}/0` (202 with `nextSubscribeUrl`, 30 s).
4. **End**: `POST …/conv/{id}/leave` → 204.

## Getting Echo into the call

`epconv` with `UserInitiatedTestCall` starts a conversation that holds only us; the roster
shows one participant. Once media connects, the web client POSTs the conversation's
`links.addParticipantAndModality` (`…/conv/{id}/add`) with `participants.to =
[{"id": "28:cf28171e-…", "participantId": <new uuid>}]`, `groupChat.threadId` = the 1:1 chat
with Echo, and `addParticipantSuccess/Failure` callback links. Echo joins
(`addParticipantSuccess`) and its audio starts. Every request of one call carries the same
`participants.from.participantId`.

## Trouter (verified from NotTeams)

1. `GET https://go-eu.trouter.teams.microsoft.com/v4/a?epid={endpointId}` with header
   `x-skypetoken` → `socketio` (host), `surl` (callback base, `…:3443/v4/f/{id}/`), `ccid`,
   `connectparams` (signed `sr`, `sp`, `se`, `st`, `sig`, …), `registrarUrl`. (The account's
   `regionGtms.calling_trouterUrl` names the host; `/v3/c` answers 400.)
2. socket.io 0.9 handshake `GET {socketio}/socket.io/1/?v=v4&tc=…&timeout=40&epid&ccid&cor_id
   &con_num&auth=true&{connectparams}` → `sid:hb:close:transports`.
3. WebSocket `wss://{host}/socket.io/1/websocket/{sid}?{same query}`. Server says `1::`; send
   `5:1+::{"name":"user.authenticate","args":[{"headers":{"Authorization":"Bearer <spaces
   token>",…},"connectparams":{…}}]}` → `5:1::{"name":"trouter.connected",…}`.
4. Pings `5:{n}+::{"name":"ping"}` → `6:::{n}+["pong"]`; heartbeats `2::` are echoed.
5. Bodies may be gzip+base64 (`X-Microsoft-Skype-Content-Encoding: gzip`).

Registration with the registrar (needed for incoming calls and pushed messages) isn't done
yet; outgoing calls don't need it, their callback URLs go straight to `surl`.

## The answer and its acknowledgement

`…/call/acceptance/` carries `callAcceptance.mediaContent.blob` (the answer, in sdp-ngc) and
`links` (`callLeg`, `mediaRenegotiation`, `applyChannelParameters`…). The client acknowledges
in the Trouter response body: `{"callAcceptanceAcknowledgement":{"links":{…its own callback
URLs…}}}`.

## SDP dialect (`application/sdp-ngc-1.0`)

Answer → standard: `RTP/SAVP` → `UDP/TLS/RTP/SAVPF`; drop `tcp-pass`/`tcp-act` candidates (or
rewrite to `tcp … tcptype passive|active`) and the `MTURNID n` suffix; extmap URIs use `\`
for `/`; `RED/8000` → `red/48000/2`; `a=x-ssrc-range:N-N` → `a=ssrc:N cname:…` (without it the
receiver never maps Teams' audio SSRC to the track); other `a=x-*` lines can go.
Offer → ngc: `b=CT:4000`, `RTP/SAVP`, port and `c=` from the first host candidate, extmap
`/` → `\`, plus `a=label:main-audio`, `a=x-ssrc-range:S-S`, `a=x-signaling-fb:* x-message app
recv:dsh`. Code: `src/sdp_ngc.rs`.

## Tokens

- `epconv`, `add`, `leave`: Bearer for `https://ic3.teams.office.com` (the spaces token gets
  403 "Unauthenticated identity").
- Trouter connect: `x-skypetoken`; Trouter authenticate: Bearer for `api.spaces.skype.com`.
- Relay (TURN) credentials: `GET https://teams.cloud.microsoft/trap/tokens` with
  `x-skypetoken` → `{"realm":"rtcmedia","username","password","expires":604800}`. Relay host
  `gateway-eu.az.relay.teams.cloud.microsoft` for EMEA (`gateway-us…` Americas,
  `worldaz.relay.teams.microsoft.com` otherwise), `turn:{host}:3478?transport=udp`.

## Media

- Plain WebRTC (DTLS-SRTP, ICE, BUNDLE; 13 m-lines in the web client's offer: audio, video,
  screen sharing, data). Teams' answer uses its own extensions (`a=x-mediabw:…`,
  `b=CT:…`) and Microsoft media servers (e.g. `c=IN IP4 52.115.132.191`).
- ICE servers: TURN relays `gateway-{eu}.az.relay.teams.cloud.microsoft` (UDP 3478, TCP 443,
  TURNS 443) with credentials from `/trap/tokens` (see Tokens).

## Incoming calls (verified from NotTeams, 2026-11-18)

Found by probing: the service's validation errors name the fields it wants.

1. **Register**: `POST {registrarUrl from trouter v4/a}` (e.g.
   `https://communications.svc.cloud.microsoft/registrar/eu/v3/registrations`) with
   `x-skypetoken` and
   `{"clientDescription":{"appId":"NextGenCalling","templateKey":"DesktopNgc_2.3:SkypeNgc",
   "platform":"edge","languageId":"en-US","aesKey":"","platformUIVersion":…},
   "registrationId":<endpoint id>,"nodeId":"","transports":{"TROUTER":[{"context":"",
   "path":"{surl}NGCallManagerWin","ttl":86400}]}}` → 202.
2. **The ring**: Trouter delivers `POST …/NGCallManagerWin` with `{callId, callerId, evt, cp}`.
   - `evt` 109 is the ring itself. `cp` is base64 of gzipped JSON holding
     `callNotification.from` (the caller), `callNotification.links.{attach, progress, reject}`,
     `conversationInvitation.conversationController` and `groupChat.threadId`. There's no media
     in it.
   - `evt` 115 comes just before the ring, with the conversation's details.
   - `evt` 116 comes when it stops ringing.
3. **Ringing**: `POST progress` with `{"callProgress":{}}` → 202.
4. **Decline**: `DELETE reject` with `{"callEnd":{"reason","code":603,…}}`. POST and PUT get
   405.
5. **Answer**, in four steps:
   1. `POST attach` with `{"attach":{"links":{progress, mediaAnswer, acceptance, redirection,
      end}}}` (our Trouter URLs) → `callInvitation.links.{newOffer, acceptance, callLeg…}`.
   2. `POST newOffer` with `{"mediaOfferRequirements":{"compatibleContentTypes":
      ["application/sdp-ngc-1.0"],"links":{"offerReady":<our URL>}}}`. The media processor's
      **offer** arrives at `offerReady` as `mediaOfferReady.mediaContent.{blob, mediaLegId}`.
   3. `POST acceptance` with `{"callAcceptance":{"acceptedCallModalities":["audio"],
      "links":{…},"mediaContent":{"contentType","blob":<our answer>,"mediaLegId":<theirs>,
      "newOffer":false}}}` → `callAcceptanceAcknowledgement.links.callLeg`.
   4. Hang up: `DELETE callLeg` with `callEnd`, and `POST {conversationController}/leave`.
   - Sending our own offer in `acceptance` instead gets "SdpParsingFailure"; it wants an
     answer.
6. **The offer** is the one Teams' desktop clients get, whatever capabilities we claim:
   - `RTP/AVP` with both `a=crypto` and a DTLS fingerprint;
   - two-component candidates;
   - an extra `m=x-data` section;
   - codecs SATIN, SILK, G.722, SIREN, G.729 and G.711, **no Opus**.

   NotTeams answers with G.711 μ-law (`src/g711.rs`), declines `x-data` with port 0, and
   must add `a=sendrecv` (the offer has no direction; without it the WebRTC stack answers
   with no audio section, which Teams rejects as "InvalidNumberOfMedia"). The conversion is
   in `sdp_ngc::offer_from_mp` / `answer_to_ngc`.
7. **End of an answered call**: there are no roster callbacks. `GET {conversationController}`
   returns `roster.participants[mri].{state, endpoints}`, which NotTeams polls every 4 s.
8. **Testing without a second person**: `notteams --diagnose answer 40` registers a fresh
   endpoint and answers. `--diagnose testcall 25 selfring` calls Echo, then invites my own
   account. **Every Teams app signed in to the account rings too**, so use it sparingly.

## Video (verified from NotTeams, 2026-11-18)

- **Codec**: Teams' media processor answers video with H.264 only (payload type 107,
  `profile-level-id=42C01E;packetization-mode=1`). NotTeams offers
  `profile-level-id=42c01f;packetization-mode=1` with nack, PLI and FIR feedback, and
  encodes with OpenH264 (Baseline, 600 kbit/s, 15 fps, a keyframe every 3 s).
- **Offer in sdp-ngc**, per video section:
  - `a=label:main-video`;
  - `a=x-ssrc-range:S-S`;
  - `a=x-signaling-fb:* x-message app send:src recv:src,vc`.

  In `epconv`, `callModalities` becomes `["Audio","Video"]` and the video section gets
  `mediaDescriptions.descriptions [{"mid","direction":"sendrecv","label":"main-video"}]`.
- **Verified with Echo** (`notteams --diagnose testcall 18 video`): the answer accepts the
  video section, and our video goes out at about 70 kB/s.
- **Receiving video is untested.** A received H.264 track is reassembled (`video.rs`,
  RFC 6184) and decoded into the window, but Teams likely sends a participant's video only
  after a subscription through its RTCP `x-message` "vc" (video control) channel, which
  isn't implemented. Testing it needs someone with a camera on the other end.
- **Incoming calls stay audio-only**, because their desktop-style offer has no H.264
  section we take.
- **Camera**: `/dev/video0` through video4linux, YUYV (or MJPEG) at 640×480
  (`camera.rs`). `--camera-test OUT.png` grabs a frame.

## Receiving video: what the phone test showed (2026-11-18)

- **Ringing yourself** reaches your other devices, but Teams mobile offers no way to accept
  a call from yourself; the invitation times out with code 487.
- **Joining from the chat** works. `epconv` with `groupChat.threadId` makes the chat post
  `Event/Call` "call started", and the phone joined from the chat's Join button. Test it
  with `notteams --diagnose phonecall SECS join`, which starts the call in the Echo chat and
  rings nobody.
- **The phone in the roster**: `CallSignalingAgent`, streams audio 403, video 404 (`recvonly`
  until its camera is on, then `sendrecv`), applicationsharing-video 414, and **data 415**.
- **No video reached NotTeams** on its main-video section with nothing requested.
- **`controlVideoStreaming` is not the subscription.** The CC link
  `mcProxy/.../controlVideoStreaming` takes `{"controlVideoStreaming":{"globalTimeStamp",
  "controlInfo":[{"streamMsid"}]}}`. Probing with wrong-typed values shows no other fields
  (`notteams --diagnose probe URL TEMPLATE names...`), and `streamMsid: 404` returned 202 but
  no video followed. It looks like the server's start/stop for a sender's own streams. The
  web client never called it.
- **Next**: the subscription is most likely a message on the data channel. The web client
  offers `m=application … webrtc-datachannel` (mid 12) and the answer's
  `a=x-plaza-msi-range:1-100 101-200` gives our receive MSIs. To get it, record the Teams web
  client's `RTCDataChannel` messages while it receives someone's video (hook
  `RTCDataChannel.prototype.send` and `onmessage` in the CDP recorder). That needs the web
  client signed in once more.

## How the web client asks for someone's video (from its code, 2026-11-18)

Found in the web client's `calling-pluginless-*.js` (searched with the browser's debugger;
recording didn't help, see below). A **source request** carries:
- `sourceId`: the sender's video stream from the roster, `-1` to stop;
- `streamMsid`: our receiving slot, the `a=x-source-streamid` of a video section in the media
  processor's answer;
- `fmtParams`: `key=value;…` with keys like max-width, max-height, max-fps, max-fs,
  max-mbps, profile-level-id.

It goes one of three ways:

1. **Default: HTTP to the call controller's `controlVideoStreaming` link** (from
   `callAcceptance.links`):
   `{"controlVideoStreaming":{"sequenceNumber":n,"globalTimeStamp":"<Date().toString()>",
   "controlInfo":[{"control":"start","sourceId":S,"streamMsid":M,"fmtParams":"…"}]}}`.
2. **The data channel** "main-channel" (when `useApplyChannelParametersForSourceRequests`):
   `{type:"sr", controlVideoStreaming:{sequenceNumber, controlInfo:{sourceId, streamMsid,
   fmtParams, subStreamIndex}}}`.
3. **`applyChannelParameters`** with
   `{multiChannelParameter:{mids:[mid], mediaParameter:"<that JSON as a string>"}}`.

**Data-channel framing**: the payload is byte `0x01` plus a JSON array. Over the wire each
frame has a header `10 0f 92 00 | seq (2 bytes) | sender id (4, BE) 01 | receiver id (4, BE)
01`, with the media processor at `0xfffffffc`. Our id is our roster `data` stream's sourceId,
told in the ack. Messages:
- handshake: `{"type":"syn","client_capabilities":["dsh","bwe","sr","ssbwe","leave","vdclc",
  "heartbeat"]}`, answered by `ack` with `receive_capabilities` / `send_capabilities`;
- `heartbeat` every second, both ways;
- `bwe {bw}` and `dsh {history:[audio sourceIds]}` (dominant speakers), from the media
  processor.

**Why it couldn't be tested**: Teams doesn't route video between endpoints of the same
account. The web client in the same call showed "Waiting for others to join" and requested
nothing. The phone's camera switched itself off for lack of receivers. 202-accepted
requests for the phone's source 1021 brought no video. Testing needs a second person or
account.

**NotTeams now**:
- `RemoteVideos` follows the roster;
- `request_video` sends (1) for the lowest video source of another endpoint, and stops it
  when it goes;
- `sdp_ngc::from_ngc` declares the first SSRC of each `x-ssrc-range` block, so the requested
  video maps to our video track;
- the test-pattern camera (`NOTTEAMS_FAKE_CAMERA=1`) can stand in for the camera.

## One-to-one calls, as the web client makes them (recorded 2026-11-19)

Recorded with the person's web Teams calling a colleague who turned her camera on mid-call.
It differs from the conversation-and-add form NotTeams used before:

- **`epconv`**: `participants.to = [{"id": <their MRI>, "participantId": <uuid>}]`,
  `suppressDialout: false`, `groupChat: null`, `mediaContent.mediaDescriptions: null`.
  There is no `add`.
- **Each of their ringing devices answers** at our `call/mediaAnswer` link, with its sdp-ngc
  in `mediaAnswer.mediaContent.blob` and `mediaAnswer.sender.endpointId`. Then
  `call/acceptance` arrives with `acceptedBy.endpointId` and no SDP, and the matching device's
  answer is the one to apply. This is peer media through relays, not the media processor.
- **Renegotiation**: when they turn the camera on (and at other media changes), a new offer
  arrives at our `call/mediaRenegotiation` link as `mediaNegotiation.mediaContent.{blob,
  mediaLegId}` with `links.mediaAnswer` (and `rejection`). Reply to the push with 200, apply
  the offer, and POST `{"mediaAnswer":{callModalities, sender, links.mediaAcknowledgement,
  clientContentForMediaController, mediaContent:{blob, contentType, clientLocation,
  mediaLegId}}}` to that link. Their video then flows in the main-video section (mid 1),
  which turns sendrecv. `call/mediaAcknowledgement` pushes follow.
- **Their offers**: `x-ssrc-range` per section (video's `S-S+99`). The web client declares
  `a=ssrc:S` plus RTX `S+50` (FID), with no mid extension. H.264 is payload type 102
  (`42C02A`), with RTX 103. There's also an `m=x-data` section, which NotTeams declines.
- **The data channel stays silent in such calls**: no source requests. Those are for
  multiparty conversations only (`requestSource` returns early when `!multiParty`).

NotTeams now places calls to people this way:
- `CallSetup::direct()` marks the call as direct;
- `place_call` collects each device's answer;
- `renegotiate()` handles the new offers;
- `sdp_ngc::peer_offer` converts them;
- the video payload type is read from the negotiated sender and the first received packet.

The Echo test call keeps the conversation-and-add form; Echo ends direct calls at once.

## Joining a meeting going on in a chat (from the web client's code, 2026-11-19)

- **Which chats have a running call**: the aggregator's chat entries carry `activeMeetup`
  while one is going on. It has `status: "Active"`, `expiration`, `conversationType`
  (`scheduledMeeting`, ad hoc...), `conversationUrl`, `organizerId`, `tenantId`, `messageId`
  and `meetingData.meetingCode`. NotTeams keeps it as `api::Meetup` on `Chat.meetup`.
- **Joining**: the web client's `joinConversationRequest` POSTs to the same conversation
  service (`epconv`). Compared with a new call:
  - `groupChat` is `{threadId: <meeting chat>, messageId}`;
  - `groupContext` is `{id: groupId}` (teams);
  - `meetingInfo` is `{tenantId, organizerId}`, plus optional `meetingType` and
    `replyChainMessageId`;
  - `participants` has `from` only, no one to ring.

  Meetings are multiparty, so others' video comes through source requests.
- **NotTeams**: `CallSetup.meeting`, `Teams::meeting_setup`, `Command::JoinMeeting`. A
  "Meeting in progress" strip at the top of the chat list and a green Join button in the
  chat header. Untested against a live meeting (joining someone's real meeting from a
  test script was declined; the person tries it from the app).

## Who is talking (verified from NotTeams, 2026-11-19)

The media processor's mixed audio lists the voices in it as RTP contributing sources
(CSRCs), and each CSRC is the `sourceId` of that participant's audio stream in the roster
(Echo: CSRC 404, roster audio source 404). The web client reads the same through
`RTCRtpReceiver.getContributingSources()`. NotTeams keeps when each CSRC was last heard
(`VideoFeeds.talking`) and rings the tile of whoever was heard in the last 600 ms. Peer
calls carry no CSRCs, so there the other side's audio level decides.

## Others' video in meetings (verified with a guest, 2026-11-19)

Tested with an instant meeting in the person's account, the web client in it (camera and
mic off), and an anonymous guest in a headless Chrome with a fake test-pattern camera.
Three things were missing:

1. **The data channel.** In meetings the web client sends source requests only over its
   data channel; the HTTP `controlVideoStreaming` / `applyChannelParameters` links answer
   200 and do nothing. The channel is `main-channel`, created before the offer. In
   sdp-ngc its section is
   `m=x-data <port> RTP/SAVP 127 126` with `a=x-data-protocol:sctp`, `a=x-ssrc-range:S-S`,
   `a=rtpmap:127 x-data/90000`, `a=rtpmap:126 rtx/90000`, `a=fmtp:126 apt=127`,
   `a=sendrecv`, `a=rtcp-mux`, `a=label:data`, `a=sctp-port:5000`. The answer's `x-data`
   section carries no `sctp-port` and becomes `m=application ... UDP/DTLS/SCTP
   webrtc-datachannel` for the WebRTC stack. Frames: `10 0f 92 | seq u16 BE | 00 | sender
   u32 BE | 01 | receiver u32 BE | 01` + a JSON array of one object. `syn` goes out as
   `0xfffffffe`; the `ack` names our id (the answer's `x-data` stream id) as receiver.
   Heartbeats every second. The web client's request:
   `{"type":"sr","controlVideoStreaming":{"sequenceNumber":n,"controlInfo":{"sourceId":415,
   "streamMsid":202,"fmtParams":[{"max-fs":8160,"max-mbps":244800,"max-fps":3000,
   "profile-level-id":"64001f"}]}}}`, answered by `sr_res` `ok`. The media processor also
   sends `dsh` (dominant speakers), `bwe` and `vdclc` (limits for our own camera).
2. **The roster at join.** Roster pushes are deltas. A latecomer learns who's already
   there from the `epconv` response's `roster` (every participant with their streams),
   not from the pushes.
3. **The SSRC.** The media processor sends a section's stream on the first SSRC of its
   `x-ssrc-range` (RTX 50 above when negotiated, as `a=ssrc-group:FID`). The WebRTC
   stack drops SSRCs the SDP doesn't declare unless they carry a simulcast rid, so
   `from_ngc` declares it.

With all three, NotTeams received the guest's video (426×240, about 15 fps) and shows it
in their tile. `notteams --diagnose meetingjoin <secs> <chat title>` joins a meeting and
reports frames. `NOTTEAMS_JOIN_MEETING=<title>` makes the window join it at start.

## Raise hand and reactions (recorded 2026-11-19)

All three go to the conversation's links from the `epconv` response, with our `from`
participant:
- **Raise**: `publishState` with `{"publishedState":{"content":{"skinTone":0},"level":"user",
  "stateType":"raiseHands","sequenceNumber":n}}`; the response's
  `publishStateResponse.stateId` is needed to lower it. The hand belongs to the user, so
  all their devices in the meeting show it.
- **Lower**: `removeState` with `{"sequenceNumber":n+1,"scope":"specified","stateIds":[id]}`.
- **Reaction**: `sendMessage` with `{"messageContent":[{"operationId":uuid,"payload":
  {"values":[{"id":uuid,"name":"like","count":1,"attributes":{"skinTone":0}}]},"scope":"all",
  "to":[],"type":"Reaction"}]}`. Names: like, heart, applause, laugh, surprised.
- **Others'**: a raised hand is in the participant's roster entry as `publishedStates`
  `[{stateType:"raiseHands", stateId,...}]`; reactions arrive at our `receiveMessage` link
  with the same `type`/`payload`.

`notteams --diagnose meetingjoin <secs> <title> raise` raises the hand and sends a like.

## Screen sharing (recorded and verified with a guest, 2026-11-19)

- **The section**: the offer carries a video section labelled `applicationsharing-video`
  (send-and-receive in the SDP, `recvonly` in `mediaDescriptions`) from the start.
- **Seeing someone's share**: their roster entry gets a stream of type
  `applicationsharing-video` (sendonly/sendrecv) with a `sourceId`. It's asked for like a
  camera, with an `sr` on the data channel into our share section's `x-source-streamid`
  (the web client lists AV1 and H.264 in `fmtParams`; NotTeams asks for H.264). Verified: a
  guest's 1920×1080 share arrived.
- **Sharing**: frames go out on that section, and the call leg's
  `updateMediaDescriptions` link (from `callAcceptance.links`) gets
  `{"UpdateMediaDescriptions":{"mediaDescriptions":{"descriptions":[...camera sendrecv
  main-video, slots recvonly, {"mid":share,"direction":"sendonly","label":
  "applicationsharing-video"}],"negotiationTag":"<uuid>;ss_1","requestId":n}}}`. Stopping
  sends the share section back as `recvonly` without a label or tag. Verified: the guest's
  Teams showed NotTeams' test picture as the main stage.
- **Capture** (`src/screen.rs`): the desktop's ScreenCast portal (GNOME's own picker) via
  `ashpd`, its PipeWire stream read by `gst-launch-1.0 pipewiresrc fd=3 path=<node>` as RGBA
  1280×720 at 10 fps, encoded by OpenH264 in screen-content mode at 1.5 Mbit/s.
  `NOTTEAMS_FAKE_SCREEN=1` shares a test picture instead;
  `notteams --diagnose meetingjoin <secs> <title> share` shares once connected.

## Calls to a person that didn't ring their desktop (2026-11-19)

A call from NotTeams rang her phone but not her PC, and none of her devices sent the
`call/mediaAnswer` each ringing device sends for the web client's calls. Differences from
the web client's offer, now fixed:
- **No relay candidate.** The Teams relay answers a TURN Allocate with `300 Try Alternate`
  and an `ALTERNATE-SERVER`, which webrtc-rs doesn't follow, so NotTeams offered only private
  and Docker-bridge addresses (`c=IN IP4 172.17.0.1`). `turn_server` now follows the redirect
  with an unauthenticated Allocate before the call (52.114.227.10 → 52.114.235.110...) and
  gives the stack the final server; the offer's m/c lines carry relay > public > host.
- **Fingerprint** only at session level; the web client puts it in every section.
- A second `main-video` section (an unused receive slot) in person-to-person offers.
- Component-2 candidates and `a=end-of-candidates`, which the web client doesn't send.
- `requiredFeatures: "nonByPass"`, `clientLocation` and the bandwidth seed in `mediaContent`.

Hanging up while it rang sent nothing (the leave link was only kept once answered), so the
other side rang until the invitation expired. Leaving before an answer now carries the web
client's cancel: `callTransactionEnd.code` 487. The call log notes each device that answers
while ringing (`ringing: device ... prepared its answer`).

Even with those fixed, her devices never sent a media answer to NotTeams' own offer (her
phone rang regardless, her PC didn't). Calls to a person therefore use the
conversation-and-add form again, where Teams' media processor makes the offer to her devices,
as in the calls that rang and connected before; her video comes through source requests on
the data channel.

**2026-11-24:** the group form shows to the other side as a group call ("meeting"), so calls to
a person are direct again, with an offer laid out like the web client's 1:1 offer (recorded
2026-11-18: audio, main video, a screen-share section and an `x-data` data channel, no receive
slots; `allowConversationWithoutHost: true`, no `mediaDescriptions` key). If no device sends a
media answer within 7 s (`NO_DEVICE_ANSWER`), the attempt ends (which withdraws the
invitation) and the call rings again in the group form. `NOTTEAMS_GROUP_CALLS=1` always uses
the group form.

**Incoming 1:1 call from Teams desktop, recorded at the web client (2026-11-24).** The callee
gets Trouter `evt` 107 with `gp` (base64 JSON): `callNotification` with the caller, `links`
(`attach`, `progress`, `reject`, `mediaAnswer: "cc://ma"`), and `mediaContent` with the
desktop's offer as `application/sdp-ngc-0.5`: only audio and main-video (`recvonly` with the
camera off), mids `audio_0`/`video_1`, no screen-share or data section, 16 audio codecs.
`conversationInvitation.isMultiParty: false`. The web callee attaches (`cc/v1/forked/...`),
reports progress (`cc/v1/incoming/.../progress`), and on pickup accepts with its answer
(`cc/v1/incoming/...`, then `negotiations`). It sends no early media answer: with
`mediaAnswer: "cc://ma"` the call controller carries the answer. So extra sections aren't what
a 1:1 needs; NotTeams' direct call still gets no media answer from any device (19 and 24 Nov).


## Meeting lobby (verified with a guest, 2026-11-19)

- Someone waiting in the lobby is in the roster deltas like everyone else (`state: "active"`),
  but their endpoint has a `lobby` entry instead of `call` and `modalityJoined: "Lobby"`;
  `participantCounts.lobbyParticipants` counts them. `Roster::apply` keeps them apart
  (`waiting`) and `VideoFeeds::lobby` shows them in the people panel.
- `getAllParticipants` with `scope: "lobby"` answered an empty map while a guest waited, so the
  roster is the source.
- Letting them in: `POST conv_links.admit` with
  `{"participants": {"from": <us>, "to": [{"id": <mri>}]}, "links": {"admitFailure", "admitSuccess"}}`.
  The next delta shows their endpoint under `call` with media streams.
- `notteams --diagnose lobbytest <secs> <thread>` joins a meeting I organise and admits
  whoever knocks.

## What the media processor asks of our camera (from a real meeting, 2026-11-20)

- On the data channel the media processor sends `{"type": "vdclc", "sourceId": <our camera's
  stream>, "fmtParamsList": ["KeyFrame=1;max-br=208;max-fps=1500.000000;max-fs=405;..."]}`:
  the largest picture (`max-fs` in 16×16 macroblocks), bitrate (kbit/s) and frame rate
  (hundredths) the people watching can take. With several people it asked for 405 and then 240
  macroblocks (about 368×272 and 272×208 from our 4:3 camera) at about 200 kbit/s. A larger
  picture than asked isn't forwarded: others saw an empty tile until only one of them was left.
  `video::SendLimit` follows it (`datalink.rs` keeps the request, the encoder scales to it).
- Others' video carries the sender's source id as the RTP CSRC. A slot given to someone else
  still delivers the previous person's frames for a moment, so pictures are tagged with it and
  shown only under the person the slot is meant for (`Frame::source`).

**What the web client sends and gets in a meeting (WebRTC stats, 2026-11-24).** Its camera is a
640×360 (16:9) source sent as simulcast: 320×180 at 15 fps (~180 kbit/s) and 640×360 at 30 fps
(~800 kbit/s), H.264 through OpenH264; the media processor forwards the layer each viewer
asked for. Others' cameras arrive at 320×180 for small tiles; a colleague's shared screen
arrived at 1920×1080, 29 fps. NotTeams sends one 640×360 layer (16:9 since this day) scaled to
what `vdclc` asks, and shares the screen at up to 1920×1080.

**Why no device answered NotTeams' direct call (found 2026-11-24).** Bisected by placing calls
from the web client with parts of NotTeams' request swapped in (CDP request interception):
NotTeams' token, callback links (to a NotTeams Trouter connection) and request fields all
worked; its real offer didn't. The WebRTC stack gave its public candidates from two local
addresses one shared foundation, and its relay candidates another; with a foundation per
candidate (`sdp_ngc::unique_foundations`) the callee's phone and desktop answered at once.

**Their screen share in a direct call (recorded 2026-11-25).** When a Teams desktop starts
sharing in a 1:1 call it renegotiates: its camera section turned off (`m=video 0 ... 34`, no
`a=mid`), the `applicationsharing-video` section `sendonly` with its `x-ssrc-range`; stopping
turns that section off too. `sdp_ngc::peer_offer` now restores a missing mid from the section's
position, bundles only sections in use, and reports the share's SSRC, which `renegotiate` maps
to the screen slot (cleared when the share stops). The roster of a 1:1 call doesn't mark the
sharer, so the stage shows the other person's screen while its picture arrives.

**Direct calls: incoming, and viewing a share (recorded 2026-11-25).** A person's 1:1 call rings
only endpoints registered the web client's way: appId `SkypeSpacesWeb`, template
`SkypeSpacesWeb_2.6`, path = the Trouter connection itself (pushes arrive at `.../v4/f/<id>/`
with `evt` 107 and the notice plain in `gp`; 116 with `cp.correlationId` when it stops). The
listener registers that besides the desktop-style one (`incoming::web_calling_endpoint`). The
web callee attaches, reports progress and accepts with an answer to the caller's own offer
(`sdp-ngc-0.5` from a desktop), no media processor in between. When the other side starts
sharing, the web client's media answer says `callModalities: ["Audio", "ScreenViewer"]`;
NotTeams said "Audio, Video", and the share was never sent to it.

## State in NotTeams

Working:
- **Calling a person** from a one-to-one chat's Call button. This uses the same flow as the
  test call, without the `UserInitiatedTestCall` scenario: `epconv` with `to: []`, then
  `add` the person with the 1:1 chat as `groupChat.threadId` once our media is up.
- **Echo test call** (Settings → Calls). Verified live: `notteams --diagnose testcall 35
  person` puts Echo through the person path, and Echo joins and talks.
- **Call progress** from the roster deltas (`participants[mri].state`): `active` means
  joined (or `addParticipantSuccess`), `inactive` means left.
- **Failed invitations**: `addParticipantFailure` ends the call with a reason from its code
  (603 declined, 486 busy, 408/480/487 no answer). The exact body shape is still unseen;
  `NOTTEAMS_CALL_DEBUG=1` logs it.
- **No answer**: after 60 s of ringing the call ends.
- Audio both ways, mute, hang up, and leaving the call when the app quits.

Code: `src/trouter.rs`, `src/call.rs`, `src/audio.rs`, `src/sdp_ngc.rs`, `src/call_ui.rs`.

Debugging: `NOTTEAMS_CALL_DEBUG=1` logs pushes, RTP and transport stats.

Also working: **incoming calls**. While NotTeams runs, its own Trouter connection is
registered for calls (`src/incoming.rs`). A ring shows a card with Accept and Decline and
plays the desktop's incoming-call sound, and the window comes back from the tray. Answered
calls are audio-only G.711.

Untested:
- With a real person on either end.
- Hanging up an answered call (`DELETE callLeg`).
- Withdrawing a ring when we hang up early: calls to people now set
  `allowConversationWithoutHost: false`.

Next: Opus for incoming calls (find what makes the media processor offer the web set), group
calls, video and meetings.
