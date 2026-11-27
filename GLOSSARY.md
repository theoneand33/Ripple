# Glossary

Terms the code names modules and types after.

- **Island**: the bar itself, and its state (`island.rs`). It rests in one of three modes: **Still**, **Quick** or **Large**.
- **Tab**: one Large Mode page (`tabs/`). A tab shown as an Overview tile is a **panel**.
- **Widget**: what an Overview tile shows, either one made for the dashboard or a tab's panel.
- **Alert**: what the compact Island shows in place of the time for a while: a level, battery, a device, a reminder.
- **Reminder**: an alert with a title, from a task, an event, an agent or a script. Sources **offer** one, and the Island holds it until it may show (`reminders.rs`). Large must be closed, it must not be quiet hours, and no reminder from elsewhere may be showing.
- **Quiet hours**: the times, set by the user, when reminders wait and downloads don't show.
- **Live activity**: something running that the compact Island keeps showing: the Pomodoro, a download, Caps Lock. Ones that don't fit beside the Island take a **bubble**.
- **Rates**: the exchange rates that Calculator, Search and Stocks share (`calc.rs`).
- **Choice**: a setting that is one of a fixed set, such as `Position`, `FullscreenRule` or `WeatherUnit`. Each value has an id saved in `prefs.json` and a label.
- **Section**: one part of a tab's Settings page. Tabs that share a feature, such as Weather or Nextcloud, share its section.
- **WindowId**: a window to give focus to, as the system that named it knows it (`platform.rs`).
- **Report**: what a download source last said of a download: running, with its share if known, or finished (`downloads.rs`).
