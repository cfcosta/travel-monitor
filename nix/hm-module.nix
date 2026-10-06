# The home-manager module (DECISIONS.md 0037): installs the daemon `tmd`
# and runs it as the systemd user service `travel-monitor`, with its own
# headless Chromium (0021). It also installs the app `tm` (0040) and
# points it at the same database and Chromium profile.
#
# The daemon is started as:
#
#   tmd --db <databasePath>
#       --profile-dir <profileDir>
#       --chromium <chromium.package>/bin/chromium
#       [--headed]                      (when chromium.headless = false)
#       --cdp-port <chromium.port>      (0: the daemon picks a free port)
#       --budget <budget.daily>
#       --max-budget <budget.max>
#       <extraArgs...>
#
# These flags are the daemon's CLI contract; keep them in sync with
# crates/tm-daemon.
#
# The service must run without a login session, which needs lingering.
# home-manager cannot enable it; on NixOS set
# `users.users.<name>.linger = true`. See docs/nix.md.
{ self }:
{
  config,
  lib,
  pkgs,
  specialArgs,
  ...
}:
let
  inherit (lib)
    literalExpression
    mkEnableOption
    mkIf
    mkOption
    optional
    types
    ;

  cfg = config.services.travel-monitor;
  system = pkgs.stdenv.hostPlatform.system;

  # Set when home-manager runs as a NixOS module; null when standalone.
  osConfig = specialArgs.osConfig or null;
  lingerEnabled =
    osConfig != null && (osConfig.users.users.${config.home.username}.linger or false) == true;

  chromiumBin = lib.getExe cfg.chromium.package;

  args = [
    "--db"
    cfg.databasePath
    "--profile-dir"
    cfg.profileDir
    "--chromium"
    chromiumBin
  ]
  ++ optional (!cfg.chromium.headless) "--headed"
  ++ [
    "--cdp-port"
    (toString cfg.chromium.port)
    "--budget"
    (toString cfg.budget.daily)
    "--max-budget"
    (toString cfg.budget.max)
  ]
  ++ cfg.extraArgs;

  # What both programs read: the app finds the daemon's database and
  # profile through these (0040, 0024).
  sharedEnvironment = {
    TM_DB = cfg.databasePath;
    TM_PROFILE_DIR = cfg.profileDir;
    TM_CHROMIUM = chromiumBin;
  };

  serviceEnvironment = sharedEnvironment // { RUST_LOG = cfg.logLevel; } // cfg.environment;

  # A script, so arguments are quoted by the shell rather than by systemd's
  # own rules (which expand `%` and `$`). It also creates the directories
  # the daemon writes to.
  start = pkgs.writeShellScript "travel-monitor-start" ''
    ${pkgs.coreutils}/bin/mkdir -p \
      ${lib.escapeShellArg (dirOf cfg.databasePath)} \
      ${lib.escapeShellArg cfg.profileDir}
    exec ${lib.getExe cfg.package} ${lib.escapeShellArgs args}
  '';

  # The app as installed: the same variables as the service, baked in, so
  # it finds the database even when started outside a login shell (where
  # home.sessionVariables are not loaded).
  app = pkgs.symlinkJoin {
    name = "tm-${cfg.appPackage.version or "configured"}";
    paths = [ cfg.appPackage ];
    nativeBuildInputs = [ pkgs.makeWrapper ];
    postBuild = ''
      wrapProgram $out/bin/tm ${
        lib.concatStringsSep " " (
          lib.mapAttrsToList (
            name: value: "--set-default ${name} ${lib.escapeShellArg value}"
          ) sharedEnvironment
        )
      }
    '';
    meta.mainProgram = "tm";
  };
in
{
  options.services.travel-monitor = {
    enable = mkEnableOption "the travel monitor daemon (`tmd`) as a systemd user service";

    package = mkOption {
      type = types.package;
      default = self.packages.${system}.tmd;
      defaultText = literalExpression "travel-monitor.packages.\${system}.tmd";
      description = "The daemon package. It must provide `bin/tmd`.";
    };

    appPackage = mkOption {
      type = types.package;
      default = self.packages.${system}.tm;
      defaultText = literalExpression "travel-monitor.packages.\${system}.tm";
      description = "The app package. It must provide `bin/tm`.";
    };

    installApp = mkOption {
      type = types.bool;
      default = true;
      description = ''
        Whether to install the app (`tm`) into `home.packages`, with a
        desktop entry. The installed `tm` has `TM_DB`, `TM_PROFILE_DIR` and
        `TM_CHROMIUM` set to this module's values.
      '';
    };

    databasePath = mkOption {
      type = types.str;
      default = "${config.xdg.dataHome}/travel-monitor/tm.db";
      defaultText = literalExpression ''"''${config.xdg.dataHome}/travel-monitor/tm.db"'';
      description = ''
        The SQLite database. The daemon writes it; the app reads it and
        writes configuration and requests. Its directory is created at
        start-up.
      '';
    };

    profileDir = mkOption {
      type = types.str;
      default = "${config.xdg.dataHome}/travel-monitor/chromium";
      defaultText = literalExpression ''"''${config.xdg.dataHome}/travel-monitor/chromium"'';
      description = ''
        The Chromium user data directory of the daemon's browser. Miles
        program sessions are kept here after a manual login from the app
        (DECISIONS.md 0024).
      '';
    };

    chromium = {
      package = mkOption {
        type = types.package;
        default = pkgs.chromium;
        defaultText = literalExpression "pkgs.chromium";
        description = "The Chromium the daemon starts (and the app opens for logins).";
      };

      headless = mkOption {
        type = types.bool;
        default = true;
        description = "Run the daemon's Chromium headless. When false, `--headed` is passed.";
      };

      port = mkOption {
        type = types.port;
        default = 0;
        description = "The CDP port of the daemon's Chromium. 0 lets the daemon pick a free port.";
      };
    };

    budget = {
      daily = mkOption {
        type = types.ints.positive;
        default = 300;
        description = ''
          The page-load budget the daemon starts with each day. It is raised
          step by step while no block is seen (DECISIONS.md 0039).
        '';
      };

      max = mkOption {
        type = types.ints.positive;
        default = 2000;
        description = "The highest daily page-load budget the daemon may ramp up to.";
      };
    };

    logLevel = mkOption {
      type = types.str;
      default = "info";
      example = "info,tm_browser=debug";
      description = "The daemon's `RUST_LOG` filter.";
    };

    extraArgs = mkOption {
      type = types.listOf types.str;
      default = [ ];
      description = "Extra arguments appended to the daemon's command line.";
    };

    environment = mkOption {
      type = types.attrsOf types.str;
      default = { };
      description = "Extra environment variables for the service. They override the module's own.";
    };
  };

  config = mkIf cfg.enable {
    assertions = [
      {
        assertion = cfg.budget.daily <= cfg.budget.max;
        message = "services.travel-monitor.budget.daily (${toString cfg.budget.daily}) must not exceed budget.max (${toString cfg.budget.max}).";
      }
    ];

    warnings = optional (osConfig != null && !lingerEnabled) ''
      services.travel-monitor: the daemon must keep running without a login
      session (DECISIONS.md 0037), which needs lingering. Set
      `users.users.${config.home.username}.linger = true;` in the NixOS
      configuration.
    '';

    home.packages = [ cfg.package ] ++ optional cfg.installApp app;

    home.sessionVariables = sharedEnvironment;

    xdg.desktopEntries = mkIf cfg.installApp {
      travel-monitor = {
        name = "Travel Monitor";
        genericName = "Airfare monitor";
        comment = "Fares, deals and settings of the travel monitor";
        exec = "${lib.getExe app}";
        terminal = false;
        categories = [ "Utility" ];
      };
    };

    systemd.user.services.travel-monitor = {
      Unit = {
        Description = "Travel monitor daemon (tmd)";
        After = [ "network-online.target" ];
      };

      Service = {
        ExecStart = "${start}";
        Environment = lib.mapAttrsToList (
          name: value: ''"${name}=${lib.escape [ "\\" "\"" ] value}"''
        ) serviceEnvironment;

        Restart = "on-failure";
        RestartSec = 30;
        # Chromium's processes live in the service's cgroup and stop with it.
        KillMode = "control-group";
        TimeoutStopSec = 30;

        # Hardening that leaves Chromium's own sandbox working: no
        # PrivateUsers, RestrictNamespaces or MemoryDenyWriteExecute (V8
        # needs writable-executable memory), and nothing that implies a user
        # namespace in a user service (ProtectSystem, ProtectHome,
        # PrivateTmp, ...).
        NoNewPrivileges = true;
        LockPersonality = true;
        RestrictRealtime = true;
        RestrictSUIDSGID = true;
        SystemCallArchitectures = "native";
        UMask = "0077";
      };

      Install.WantedBy = [ "default.target" ];
    };
  };
}
