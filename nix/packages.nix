# The two binaries (DECISIONS.md 0017, 0045): the daemon `tmd` and the
# gpui app `tm`. Both are built from the whole workspace, so they share
# one source and one Cargo.lock.
{
  lib,
  makeRustPlatform,
  makeWrapper,
  pkg-config,
  chromium,
  rust,
  guiLibs,
  src,
}:
let
  rustPlatform = makeRustPlatform {
    cargo = rust;
    rustc = rust;
  };

  common = {
    version = "0.1.0";

    src = lib.fileset.toSource {
      root = src;
      fileset = lib.fileset.unions [
        (src + "/Cargo.toml")
        (src + "/Cargo.lock")
        (src + "/crates")
      ];
    };

    cargoLock.lockFile = src + "/Cargo.lock";

    # tm-store's sqlx query metadata is committed (crates/tm-store/.sqlx);
    # no database at build time.
    SQLX_OFFLINE = "true";

    nativeBuildInputs = [
      pkg-config
      makeWrapper
    ];

    meta = {
      platforms = lib.platforms.linux;
    };
  };
in
{
  tmd = rustPlatform.buildRustPackage (
    common
    // {
      pname = "tmd";

      cargoBuildFlags = [
        "--package"
        "tm-daemon"
      ];

      # The crates that need neither a browser nor a display. tm-browser's
      # integration tests drive a real Chromium (they read TM_CHROMIUM and
      # skip when it is unset); the sandbox has none, so they stay out.
      cargoTestFlags = [
        "--package"
        "tm-domain"
        "--package"
        "tm-gflights"
        "--package"
        "tm-scheduler"
        "--package"
        "tm-store"
        "--package"
        "tm-jev"
      ];
      preCheck = ''
        unset TM_CHROMIUM
      '';

      # The daemon starts its own Chromium (0021). The home-manager module
      # passes `--chromium` explicitly; this default is for running it by
      # hand.
      postFixup = ''
        wrapProgram $out/bin/tmd --set-default TM_CHROMIUM ${lib.getExe chromium}
      '';

      meta = common.meta // {
        description = "Travel monitor daemon: schedules checks, drives Chromium and records prices";
        mainProgram = "tmd";
      };
    }
  );

  tm = rustPlatform.buildRustPackage (
    common
    // {
      pname = "tm";

      cargoBuildFlags = [
        "--package"
        "tm-app"
      ];

      # The shared crates are tested by `tmd`; tm-app's own tests would
      # need a display.
      doCheck = false;

      buildInputs = guiLibs;

      # gpui opens Vulkan, Wayland and X11 with dlopen, so the binary needs
      # them on its library path. The app opens the daemon's Chromium
      # profile in a visible window for miles logins (0024).
      postFixup = ''
        wrapProgram $out/bin/tm \
          --prefix LD_LIBRARY_PATH : ${lib.makeLibraryPath guiLibs} \
          --set-default TM_CHROMIUM ${lib.getExe chromium}
      '';

      meta = common.meta // {
        description = "Travel monitor app: fares, deals and settings (gpui)";
        mainProgram = "tm";
      };
    }
  );
}
