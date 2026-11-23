# Windows notification area

This library comes from the MIT-licensed [`tray-item` 0.10.0](https://crates.io/crates/tray-item/0.10.0).
The archive SHA-256 is `59d4bd406170690dc30eabb3badc67a085beaf9b2c3b1923afcc9c26a2191353`.

The Windows backend starts its event worker after the notification window opens
and exits the worker when the event channel closes. This lets Orvia retry when
Explorer or the notification area is unavailable without leaving a busy thread.
