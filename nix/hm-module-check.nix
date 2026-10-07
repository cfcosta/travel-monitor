# `nix flake check`: evaluates the home-manager module (nix/hm-module.nix)
# in a real home-manager configuration and checks the unit it renders.
# The packages are stand-ins, so the check builds neither Chromium nor
# the workspace.
{
  lib,
  pkgs,
  runCommand,
  writeShellScriptBin,
  home-manager,
  module,
}:
let
  stub = name: writeShellScriptBin name "exit 0";

  home =
    {
      extraSpecialArgs ? { },
      settings,
    }:
    home-manager.lib.homeManagerConfiguration {
      inherit pkgs extraSpecialArgs;
      modules = [
        module
        {
          home = {
            username = "tm";
            homeDirectory = "/home/tm";
            stateVersion = "25.11";
          };
          services.travel-monitor = {
            enable = true;
            package = stub "tmd";
            appPackage = stub "tm";
            chromium.package = stub "chromium";
          }
          // settings;
        }
      ];
    };

  configured = home {
    settings = {
      chromium = {
        package = stub "chromium";
        headless = false;
        port = 9333;
      };
      budget.daily = 120;
      budget.sites."airline:TP" = {
        daily = 300;
        max = 1000;
      };
      logLevel = "debug,tm_browser=trace";
      extraArgs = [ "--extra" ];
      environment.TM_EXTRA = "with space";
    };
  };

  defaults = home { settings = { }; };

  # As a NixOS submodule without lingering: the module must warn.
  noLinger = home {
    extraSpecialArgs.osConfig.users.users.tm.linger = false;
    settings = { };
  };
  withLinger = home {
    extraSpecialArgs.osConfig.users.users.tm.linger = true;
    settings = { };
  };

  unitOf = hm: hm.config.xdg.configFile."systemd/user/travel-monitor.service".source;
  startOf = hm: hm.config.systemd.user.services.travel-monitor.Service.ExecStart;

  evalChecks = [
    {
      ok = defaults.config.warnings == [ ];
      what = "standalone: no warnings";
    }
    {
      ok = noLinger.config.warnings != [ ];
      what = "NixOS without linger: a warning";
    }
    {
      ok = withLinger.config.warnings == [ ];
      what = "NixOS with linger: no warning";
    }
    {
      ok =
        defaults.config.services.travel-monitor.databasePath
        == "/home/tm/.local/share/travel-monitor/tm.db";
      what = "default database path";
    }
    {
      ok = defaults.config.home.sessionVariables.TM_DB == "/home/tm/.local/share/travel-monitor/tm.db";
      what = "TM_DB in the session";
    }
  ];
  failed = lib.filter (c: !c.ok) evalChecks;
in
assert lib.assertMsg (failed == [ ]) (
  "hm-module check failed: " + lib.concatMapStringsSep ", " (c: c.what) failed
);
runCommand "travel-monitor-hm-module-check"
  {
    # Building the activation package also enforces the module's assertions.
    activation = configured.activationPackage;
    unit = unitOf configured;
    start = startOf configured;
    defaultStart = startOf defaults;
  }
  ''
    set -euo pipefail
    fail() { echo "FAIL: $*" >&2; exit 1; }
    has() { grep -qF -- "$2" "$1" || fail "$1 lacks: $2"; }
    lacks() { ! grep -qF -- "$2" "$1" || fail "$1 has: $2"; }

    cat "$unit"
    has "$unit" "ExecStart=$start"
    has "$unit" 'After=network-online.target'
    has "$unit" 'Restart=on-failure'
    has "$unit" 'WantedBy=default.target'
    has "$unit" 'Environment="TM_DB=/home/tm/.local/share/travel-monitor/tm.db"'
    has "$unit" 'Environment="TM_PROFILE_DIR=/home/tm/.local/share/travel-monitor/chromium"'
    has "$unit" 'Environment="TM_CHROMIUM=/nix/store/'
    has "$unit" 'Environment="RUST_LOG=debug,tm_browser=trace"'
    has "$unit" 'Environment="TM_EXTRA=with space"'
    lacks "$unit" 'PrivateUsers'
    lacks "$unit" 'MemoryDenyWriteExecute'

    cat "$start"
    has "$start" "--db /home/tm/.local/share/travel-monitor/tm.db"
    has "$start" "--profile-dir /home/tm/.local/share/travel-monitor/chromium"
    has "$start" "--chromium /nix/store/"
    has "$start" "--headed"
    # Nothing else between the port, the budgets and the extra arguments.
    has "$start" "--cdp-port 9333 --budget 120 --max-budget 30000 --site-budget 'airline:TP=300:1000' --extra"

    lacks "$defaultStart" "--headed"
    lacks "$defaultStart" "--site-budget"
    has "$defaultStart" "--cdp-port 0 --budget 30000 --max-budget 30000"

    test -x "$activation/home-path/bin/tm" || fail "tm is not installed"
    test -x "$activation/home-path/bin/tmd" || fail "tmd is not installed"
    grep -qF 'TM_DB' "$activation/home-path/bin/tm" || fail "tm is not wrapped with TM_DB"
    test -e "$activation/home-path/share/applications/travel-monitor.desktop" \
      || fail "no desktop entry"

    touch "$out"
  ''
