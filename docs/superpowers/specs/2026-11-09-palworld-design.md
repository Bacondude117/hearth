# Palworld + SteamCMD — design (v0.4)

## Goal

Host Palworld dedicated servers, and lay the groundwork (SteamCMD, console-less servers) for
further Steam games.

## SteamCMD (`steamcmd.rs`)

- Bootstrapper downloaded once to `tools/steamcmd/` (Windows zip / Linux tar.gz from Valve's CDN).
- `app_update(app, dir)` runs `+force_install_dir <dir> +login anonymous +app_update <app> validate +quit`,
  parsing `Update state (0x..) <state>, progress: <pct>` for progress and `Success! App '<id>' fully installed.`
  for success. Up to 3 attempts (SteamCMD self-updates on first run). One job at a time.
- Installed build read from `steamapps/appmanifest_<app>.acf` (`buildid`).

## Palworld provider (Steam app 2394010)

- Launch `Pal/Binaries/Win64/PalServer-Win64-Shipping-Cmd.exe Pal -port=<p> -useperfthreads -NoAsyncLoadingThread -UseMultithreadForDS`
  directly so its log streams to the console.
- `Pal/Saved/Config/WindowsServer/PalWorldSettings.ini` is created from `DefaultPalWorldSettings.ini`
  with `RESTAPIEnabled=True`, `RESTAPIPort=<port+1>`, a random `AdminPassword`, and (on first
  install) `ServerName`/`PublicPort`. Hearth refuses edits that would disable its control.
- Ports: game UDP `<port>`, REST TCP `<port+1>` (both reserved and checked per protocol).
- Settings parser handles quoted strings with commas and nested `(…)` lists, preserving order.

## Console-less control (generic provider hooks)

- `poll()`: `GET /v1/api/players` every 3 s until it answers (server marked Online), then every
  10 s for the online list; Steam IDs are recorded in the player book.
- `remote_stop()`: `POST save` then `POST shutdown {waittime:1}`; Hearth then waits for the
  process (kills it if the API doesn't answer).
- `console_input()`: the console box broadcasts via `POST announce`.
- Players: `POST kick/ban {userid,message}`, `POST unban {userid}` while running; ban/unban edit
  `Pal/Saved/SaveGames/banlist.txt` while stopped. No roles/whitelist (UI hides those columns).
- playit: plain UDP tunnel (`tunnel_type: null`).

REST endpoints follow `thijsvanloef/palworld-server-docker` (Basic auth `admin:<AdminPassword>`).

## Testing

Unit: SteamCMD output and manifest parsing, settings parser (quotes, lists, injection), REST
client against a local fake HTTP server, settings preparation, ports/launch, offline bans.
Integration: full start → poll → Online → players → broadcast → kick → save+shutdown against a
fake server process and fake REST API. A real SteamCMD/Palworld install is verified on Windows by
the user (the sandbox blocks running downloaded executables).
