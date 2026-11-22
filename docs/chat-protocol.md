# Chat: live events, typing and search

## Live chat events (verified 2026-11-19)

The Trouter connection NotTeams already keeps for incoming calls (`incoming.rs`) is registered a
second time with the registrar, as `appId: "TeamsCDLWebWorker"`, `templateKey:
"TeamsCDLWebWorker_2.7"`, path `{surl}messaging`, under its own registration id (one
registration holds one app). The chat service then pushes every message event there as it
happens:

```json
{"type": "EventMessage", "resourceType": "NewMessage" | "MessageUpdate",
 "resource": {"messagetype": "RichText/Html" | "Control/Typing" | "Control/ClearTyping" | ...,
              "from": ".../contacts/8:orgid:<id>", "to": "<chat id>",
              "imdisplayname": "...", "fromDisplayNameInToken": "..."}}
```

`notteams --diagnose pushtest <secs> <appId> <template> <path> [chat]` registers a probe and
prints what arrives (types only).

Typing (`Control/Typing`) only comes with template 2.7, the web client's current one
(verified 2026-11-20 with a guest typing in a meeting chat): 2.6 delivers new messages but no
typing. The chat service's own endpoint API (`PUT v2/users/ME/endpoints/{id}`) answers
410 Gone, and today's web client skips it.

A new or changed message refreshes the chat list and the open chat at once instead of at the
next poll; `Control/Typing` shows "Name is typing…" over the compose box for 8 seconds or until
their message arrives.

## Sending typing

`POST /v1/users/ME/conversations/{chat}/messages` with
`{"content": "", "contenttype": "Application/Message", "messagetype": "Control/Typing",
"clientmessageid": <ms>}`. NotTeams sends it at most every 4 seconds while the person types
(not in their own notes, not while editing a sent message).

## Search (recorded 2026-11-19)

Web Teams' search box asks Microsoft Search:

```
POST https://substrate.office.com/searchservice/api/v2/query
Authorization: Bearer <token for https://substrate.office.com/search/.default>
x-anchormailbox / x-routingparameter-sessionkey: OID:<oid>@<tenant>
{"EntityRequests": [{"entityType": "Message", "contentSources": ["Teams"],
  "query": {"queryString": "NOT (isClientSoftDeleted:TRUE) AND <words>"}, "from": 0, "size": 25,
  ...}], "scenario": {"Name": "powerbar", ...}}
```

The Teams desktop client id gets that token. Each result's `Source` has `ClientConversationId`
(the chat; `;messageid=` for replies), `InternetMessageId` (the sender's client message id, not
the chat service's id), `From.EmailAddress.Name`, `DateTimeSent`, `Preview`, and
`HitHighlightedSummary` (plain text or HTML, no hit marks). `notteams --diagnose searchtest
<words>` prints the answer's shape only. `NOTTEAMS_FAKE_SEARCH=1` with `NOTTEAMS_OPEN_SEARCH=<words>`
shows made-up results.

In NotTeams: Enter in the top search box searches messages; the results replace the chat list,
and a result opens its chat on the right with the message scrolled to and marked. A message
older than the 50 the chat loads opens the chat without the mark.
