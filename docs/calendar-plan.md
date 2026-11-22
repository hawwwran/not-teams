# Calendar: plan (read-only first)

The Calendar item in the left rail is there but disabled. This is how it becomes a read-only
calendar, then what could follow.

## Where the data comes from (verified 2026-11-19)

Web Teams' calendar reads the Teams middle tier, not Outlook or Graph:

```
GET https://teams.cloud.microsoft/api/mt/part/{partition}/v2.0/me/calendars/default/calendarView
    ?StartDate=2026-11-17T23:00:00.000Z&EndDate=2026-11-20T23:00:00.000Z&shouldDecryptData=true
Authorization: Bearer <api.spaces.skype.com token>   (the one NotTeams already has)
x-ms-client-type: web, x-ms-migration: True
```

`Teams::calendar_view_raw` makes this call; `notteams --diagnose calendar` reads this week and
prints only the count and the field names. It returned 18 events (11 online meetings) with:

| Field | Use |
|---|---|
| `objectId`, `seriesMasterId`, `eventType` | identity; `Occurrence` / `SingleInstance`: recurring series come expanded |
| `subject`, `location`, `bodyPreview`, `organizerName` | what's shown |
| `startTime`, `endTime`, `eventTimeZone`, `utcOffset` | when (ISO times with offset) |
| `showAs` (`Busy`, `Tentative`, `Free`, `Oof`...), `myResponseType`, `isCancelled` | colour and state |
| `isOnlineMeeting`, `skypeTeamsDataObject.cid`, `skypeTeamsMeetingUrl`, `joinOnlineMeetingUrl` | the meeting's chat thread and join link |
| `categories`, `conflictingMeetings`, `isResponseRequested` | extras |

## Status (2026-11-19)

Steps 1–5 are built: `src/calendar.rs` (data, tests), `Command::LoadCalendar` /
`Event::Calendar` and a 5-minute `Event::Upcoming` for reminders in the worker,
`src/calendar_ui.rs` (view, card, tests), reminders in `ui.rs` (`REMIND_BEFORE_SECS`, setting
`meeting_reminders`). `NOTTEAMS_FAKE_CALENDAR=1` shows made-up events and
`NOTTEAMS_OPEN_CALENDAR=1` opens the view at start, for checking the look without real data.

## Steps

1. **Data** (`api.rs`): `Teams::calendar(start, end) -> Vec<CalendarEvent>` with typed fields
   (local times through chrono, all-day detected from whole-day spans, the meeting's thread id
   and join link parsed from `skypeTeamsDataObject` / `skypeTeamsMeetingUrl`). Unit tests on a
   recorded answer with the subjects replaced.
2. **Worker** (`worker.rs`): `Command::LoadCalendar { from, to }` → `Event::Calendar(Result)`.
   Loaded when the view opens and when the shown week changes; refreshed every 5 minutes while
   it's open. The last answer is kept so switching back is instant.
3. **View** (`calendar_ui.rs`, new): the Calendar rail item turns on and opens
   `View::Calendar` instead of the chat list and conversation:
   - a toolbar: Today, previous / next, the date range, and Day / Work week / Week;
   - a scrollable grid of hours (opening at 8:00) by days, a strip for all-day events on top,
     and a red line at the current time;
   - events as blocks coloured by `showAs` (busy filled, tentative striped, free outlined,
     out of office purple), cancelled ones struck through, overlapping ones side by side;
   - Teams' look: the same light theme and Fluent icons as the rest of NotTeams.
4. **Details**: clicking an event opens a card with subject, time, organizer, location, your
   response and the body preview. For an online meeting it has **Join** (the existing meeting
   join, with the thread from `skypeTeamsDataObject` and organizer/tenant from the join link's
   `context`) and **Open chat** (the meeting's chat, when NotTeams lists it).
5. **Reminder** (small, uses the new notifications): a desktop notification a few minutes before
   an online meeting starts, with a Join button. A setting to turn it off.

Each step is built, checked and shown to you before the next.

## Writing (built 2026-11-19, tested by creating and deleting events)

- **Create**: `POST v2.0/me/calendars/default/events?isOnlineMeeting=…&shouldDecryptData=false`
  with `{"calendarEvent": {subject, startTime, endTime, isAllDayEvent, location, bodyContent,
  bodyContentType: "html", attendees: [{address, name, type: "Required"}], isOnlineMeeting,
  showAs, isReminderSet, reminderMinutesBeforeStart, …}}`. Times must be UTC (`…Z`): an offset
  is ignored and the clock time read as UTC. The endpoint only makes meetings: with nobody
  invited it answers `NoAttendees`, so the organizer is added as the attendee.
- **Teams meeting**: first `POST beta/me/calendarEvents/privateMeeting/schedulingService/create`
  with `{"isStreamEnabled": false, "meetingType": "Scheduled"}`, then the create carries its
  answer (without `expiryTime`) as `schedulingServiceEvent`, as the web client does.
- **Delete**: `DELETE v2.0/me/calendars/events/{id}`; a meeting we organised is cancelled for
  everyone with `POST v2.0/me/calendars/events/{id}/cancel`.
- `notteams --diagnose eventtest [online]` creates a test event tomorrow 20:00, checks it, deletes
  it and checks it's gone.

- **Reply to an invitation**: `PUT v2.0/me/calendars/events/{id}/respond` with
  `{"responseType": "Accept" | "TentativelyAccept" | "Decline", "sendResponse": true,
  "comment": ""}`, 204 on success (read from the web client's code, not recorded, so no real
  invitation was answered). `notteams --diagnose respondtest` sends it for a made-up id: the
  service takes the request and refuses only the id (`ErrorInvalidIdMalformed`).

## Later

- Creating a meeting ("Meet now" exists in the middle tier:
  `beta/me/calendarEvents/privateMeeting/schedulingService/create`, seen in the recordings).
- Editing and cancelling own meetings.

## Open questions

- Private events from other people's calendars and shared calendars aren't in this answer.
- Time zones: times come with an offset and are shown in the computer's zone; meetings whose
  own zone differs (`eventTimeZone`) aren't converted twice. Checked against web Teams.
- How far ahead the endpoint answers in one call (a month view would need it); weeks are safe.
- Checking it in screenshots means showing your real meetings; I'll check the layout with the
  week's real data only where you agree, otherwise with made-up events.
