# Nix

The flake builds two packages and exports a home-manager module
(DECISIONS.md 0027, 0037):

- `packages.<system>.tmd`: the daemon.
- `packages.<system>.tm` (also `default`): the gpui app.
- `homeManagerModules.default`: runs `tmd` as the systemd user service
  `travel-monitor` and installs `tm`.

`nix flake check` builds both packages (with the tests of the crates
that need no browser or display) and evaluates the module in a
home-manager configuration (`nix/hm-module-check.nix`).

## Enable the module

Add the flake as an input and import the module into your home-manager
configuration:

```nix
# flake.nix
inputs.travel-monitor = {
  url = "path:/home/me/Code/travel-monitor"; # or a git URL
  inputs.nixpkgs.follows = "nixpkgs";
  inputs.home-manager.follows = "home-manager";
};
```

```nix
# home.nix
{ inputs, ... }:
{
  imports = [ inputs.travel-monitor.homeManagerModules.default ];

  services.travel-monitor = {
    enable = true;
    budget.daily = 300;
    logLevel = "info";
  };
}
```

The options are documented in `nix/hm-module.nix`. The main ones:

| Option | Default |
| --- | --- |
| `package`, `appPackage` | this flake's `tmd`, `tm` |
| `installApp` | `true` |
| `databasePath` | `$XDG_DATA_HOME/travel-monitor/tm.db` |
| `profileDir` | `$XDG_DATA_HOME/travel-monitor/chromium` |
| `chromium.package` / `.headless` / `.port` | `pkgs.chromium` / `true` / `0` (free port) |
| `budget.daily` / `budget.max` (Google Flights, 0060) | `30000` / `30000` (0077) |
| `budget.sites` (other sites, `--site-budget`) | `{ }` (300, ramping to 1,000) |
| `logLevel` | `"info"` (`RUST_LOG`) |
| `extraArgs`, `environment` | `[ ]`, `{ }` |

## Lingering

The daemon must keep running without a login session (0037). systemd
stops user services at logout unless the user lingers, and home-manager
cannot turn that on. In the NixOS configuration:

```nix
users.users.<name>.linger = true;
```

When home-manager runs as a NixOS module, the module warns if this is
not set. As a standalone home-manager it cannot tell, so check with
`loginctl show-user $USER --property=Linger`.

## Where data lives

- Database: `~/.local/share/travel-monitor/tm.db` (with its `-wal` and
  `-shm` files). The daemon writes it; the app reads it and writes
  configuration and requests (0040, 0046).
- Chromium profiles: `~/.local/share/travel-monitor/chromium/<site>`,
  one per site (`google_flights`, `airline-tp`, `miles-smiles`, …), each
  with its own browser process (0062). Miles logins are kept in the
  program's own profile (0024); logins from the shared profile used
  before 0062 are not carried over.
- Logs: `journalctl --user -u travel-monitor -f`.

Both directories are created when the service starts. The service runs
with `UMask=0077`, so the files are readable by the user only.

## The service

```sh
systemctl --user status travel-monitor
systemctl --user restart travel-monitor
```

It runs `tmd --db … --profile-dir … --chromium … --cdp-port … --budget …
--max-budget …` (plus `--site-budget site=daily:max` for each of
`budget.sites`, `--headed` when the browser is not headless, and
`extraArgs`), restarts on failure, and starts with the user manager
(`default.target`).

## Open the app

With `installApp = true` (the default), `tm` is on the `PATH` and has a
"Travel Monitor" desktop entry. The installed `tm` has `TM_DB`,
`TM_PROFILE_DIR` and `TM_CHROMIUM` set to the module's values, so it
opens the daemon's database and profile wherever it is started from.
The same variables are exported in `home.sessionVariables`.

Without the module:

```sh
nix run .#tm   # or: nix build .#tm .#tmd
```
