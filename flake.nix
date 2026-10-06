{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    treefmt-nix = {
      url = "github:numtide/treefmt-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    # Only for `checks`: the home-manager module is evaluated against the
    # real home-manager. Consumers can make it follow their own.
    home-manager = {
      url = "github:nix-community/home-manager";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      home-manager,
      nixpkgs,
      rust-overlay,
      treefmt-nix,
      ...
    }:
    let
      supportedSystems = [
        "x86_64-linux"
        "aarch64-linux"
      ];

      forEachSupportedSystem =
        f:
        nixpkgs.lib.genAttrs supportedSystems (
          system:
          f (
            let
              pkgs = import nixpkgs {
                inherit system;
                overlays = [ (import rust-overlay) ];
              };

              rust = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;

              # gpui links xcb and xkbcommon, and loads Vulkan, Wayland and
              # X11 at runtime (as in ../tau-agent).
              guiLibs = with pkgs; [
                fontconfig
                freetype
                libx11
                libxcb
                libxcursor
                libxi
                libxkbcommon
                libxrandr
                vulkan-loader
                wayland
              ];

              packages = pkgs.callPackage ./nix/packages.nix {
                inherit rust guiLibs;
                src = ./.;
              };

              formatter =
                (treefmt-nix.lib.evalModule pkgs {
                  projectRootFile = "flake.nix";
                  settings = {
                    allow-missing-formatter = true;
                    verbose = 0;
                    global.excludes = [
                      "*.lock"
                      "crates/*/.sqlx/*"
                    ];
                    formatter = {
                      nixfmt.options = [ "--strict" ];
                      rustfmt = {
                        package = rust;
                        options = [
                          "--config-path"
                          "${./rustfmt.toml}"
                        ];
                      };
                    };
                  };
                  programs = {
                    nixfmt.enable = true;
                    rustfmt = {
                      enable = true;
                      package = rust;
                    };
                    taplo.enable = true;
                  };
                }).config.build.wrapper;
            in
            {
              inherit
                formatter
                guiLibs
                packages
                pkgs
                rust
                ;
            }
          )
        );
    in
    {
      formatter = forEachSupportedSystem ({ formatter, ... }: formatter);

      packages = forEachSupportedSystem (
        { packages, ... }: {
          inherit (packages) tmd tm;
          default = packages.tm;
        }
      );

      # The daemon as a systemd user service (DECISIONS.md 0037).
      homeManagerModules.default = import ./nix/hm-module.nix { inherit self; };

      checks = forEachSupportedSystem (
        { packages, pkgs, ... }: {
          inherit (packages) tmd tm;

          hm-module = pkgs.callPackage ./nix/hm-module-check.nix {
            inherit home-manager;
            module = self.homeManagerModules.default;
          };
        }
      );

      devShells = forEachSupportedSystem (
        {
          formatter,
          guiLibs,
          pkgs,
          rust,
          ...
        }:
        {
          default = pkgs.mkShell {
            name = "travel-monitor";

            buildInputs =
              with pkgs;
              [
                formatter
                rust

                cargo-deny
                cargo-nextest
                sqlx-cli

                chromium
                curl
                jq
                sqlite
              ]
              ++ guiLibs;

            nativeBuildInputs = [ pkgs.pkg-config ];

            LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath guiLibs;
            # tm-store's queries are checked against the committed .sqlx
            # data; no database is needed to build.
            SQLX_OFFLINE = "true";
            TM_CHROMIUM = pkgs.lib.getExe pkgs.chromium;
          };
        }
      );
    };
}
