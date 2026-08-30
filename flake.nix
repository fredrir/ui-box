{
  description = "ui-box - harness-agnostic live UI testing";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
  };

  outputs =
    { self, nixpkgs }:
    let
      lib = nixpkgs.lib;

      systems = [
        "x86_64-linux"
        "aarch64-darwin"
      ];

      eachSystem = f: lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});

      version = "0.1.0";

      sourceOf =
        paths:
        lib.fileset.toSource {
          root = ./.;
          fileset = lib.fileset.unions (map lib.fileset.maybeMissing paths);
        };

      packagesFor =
        pkgs:
        let
          isLinux = pkgs.stdenv.hostPlatform.isLinux;

          tauri-driver = pkgs.rustPlatform.buildRustPackage rec {
            pname = "tauri-driver";
            version = "2.0.6";

            src = pkgs.fetchCrate {
              inherit pname version;
              hash = "sha256-fTCkEs4NLBW0khaHL4jpVNkrbQg22YPsRMjfJNqnCWA=";
            };

            cargoHash = "sha256-MThAcU+U8PyBGauh3dy7ZRvRX9INmOEeghIlQEGLAPs=";

            meta = {
              description = "WebDriver server that drives Tauri applications";
              homepage = "https://github.com/tauri-apps/tauri";
              license = with lib.licenses; [
                asl20
                mit
              ];
              mainProgram = "tauri-driver";
              platforms = lib.platforms.linux;
            };
          };

          domWrapperArgs = [
            "--set-default LIBGL_ALWAYS_SOFTWARE 1"
            "--set-default PLAYWRIGHT_BROWSERS_PATH ${pkgs.playwright-driver.browsers}"
            "--set-default PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD 1"
          ]
          ++ lib.optionals isLinux [
            "--set-default UIBOX_TAURI_DRIVER ${lib.getExe tauri-driver}"
            "--set-default UIBOX_NATIVE_DRIVER ${lib.getExe pkgs.webkitgtk_4_1}"
          ];

          uibox-vision = pkgs.python3Packages.buildPythonApplication {
            pname = "uibox-vision";
            inherit version;
            pyproject = true;

            src = sourceOf [ ./tools/vision ];
            sourceRoot = "source/tools/vision";

            build-system = [ pkgs.python3Packages.hatchling ];

            dependencies = with pkgs.python3Packages; [
              numpy
              pillow
              pyyaml
            ];

            nativeBuildInputs = [ pkgs.makeWrapper ];

            nativeCheckInputs = [
              pkgs.git
              pkgs.python3Packages.pytestCheckHook
            ];

            postFixup = ''
              wrapProgram $out/bin/uibox-vision \
                --prefix PATH : ${
                  lib.makeBinPath [
                    pkgs.git
                    pkgs.openssh
                  ]
                }
            '';

            meta = {
              description = "Screenshot diffing and golden store for ui-box";
              mainProgram = "uibox-vision";
            };
          };

          ui-box-dom = pkgs.buildNpmPackage {
            pname = "ui-box-dom";
            inherit version;

            src = sourceOf [ ./drivers/dom ];
            sourceRoot = "source/drivers/dom";

            npmDepsHash = "sha256-Tnd+sgU7OrnynFXFBucCdfuxSHQY2FKhIZ1wRVtSLXU=";

            nodejs = pkgs.nodejs_22;
            nativeBuildInputs = [ pkgs.makeWrapper ];

            PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD = "1";
            PLAYWRIGHT_BROWSERS_PATH = "${pkgs.playwright-driver.browsers}";

            postFixup = ''
              wrapProgram $out/bin/ui-box-dom \
                ${lib.concatStringsSep " \\\n    " domWrapperArgs}
            '';

            meta = {
              description = "Playwright-backed DOM driver for ui-box";
              mainProgram = "ui-box-dom";
            };
          };

          ui-box-unwrapped = pkgs.rustPlatform.buildRustPackage {
            pname = "ui-box-unwrapped";
            inherit version;

            src = sourceOf [
              ./Cargo.toml
              ./Cargo.lock
              ./crates
            ];

            cargoLock.lockFile = ./Cargo.lock;

            cargoBuildFlags = [
              "--package"
              "ui-box"
            ];

            nativeCheckInputs = [ pkgs.git ];

            meta = {
              description = "Live UI testing CLI";
              mainProgram = "ui-box";
            };
          };

          ui-box =
            pkgs.runCommand "ui-box-${version}"
              {
                nativeBuildInputs = [ pkgs.makeWrapper ];
                inherit (ui-box-unwrapped) meta;
                passthru = { inherit ui-box-unwrapped; };
              }
              ''
                mkdir -p $out/bin
                makeWrapper ${lib.getExe ui-box-unwrapped} $out/bin/ui-box \
                  --set-default UIBOX_DRIVER_DOM ${lib.getExe ui-box-dom} \
                  --set-default UIBOX_VISION ${lib.getExe uibox-vision} \
                  --set-default LIBGL_ALWAYS_SOFTWARE 1 \
                  --set-default PLAYWRIGHT_BROWSERS_PATH ${pkgs.playwright-driver.browsers} \
                  --set-default PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD 1 \
                  --prefix PATH : ${
                    lib.makeBinPath [
                      pkgs.git
                      pkgs.openssh
                      pkgs.rsync
                    ]
                  } \
                  --suffix PATH : ${lib.makeBinPath [ pkgs.nix ]}
              '';
          ui-box-adapters = pkgs.runCommand "ui-box-adapters-${version}" { src = sourceOf [ ./adapters ]; } ''
            install -Dm0755 $src/adapters/git/pre-push \
              $out/bin/ui-box-hook-pre-push
            install -Dm0755 $src/adapters/claude-code/hooks/ui-box-hook-claude-post-tool-use \
              $out/bin/ui-box-hook-claude-post-tool-use
            install -Dm0755 $src/adapters/claude-code/hooks/ui-box-hook-claude-stop \
              $out/bin/ui-box-hook-claude-stop

            mkdir -p $out/share/ui-box
            cp -r $src/adapters $out/share/ui-box/adapters
          '';
        in
        {
          inherit
            ui-box
            ui-box-unwrapped
            ui-box-dom
            ui-box-adapters
            uibox-vision
            ;
          default = ui-box;
        }
        // lib.optionalAttrs isLinux { inherit tauri-driver; };

      hostPackages = eachSystem packagesFor;
    in
    {
      packages = hostPackages // {
        x86_64-linux = hostPackages.x86_64-linux // {
          image-ui-box-backend = self.nixosConfigurations.ui-box-backend.config.system.build.image;
        };
      };

      # The lab that ui-box's graphical tests run inside. One flake, so the
      # runner module below and the system importing it resolve one nixpkgs.
      nixosConfigurations.ui-box-backend = lib.nixosSystem {
        system = "x86_64-linux";

        modules = [
          ./backend/nixos
          self.nixosModules.runner
        ];
      };

      devShells = eachSystem (
        pkgs:
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              biome
              cargo
              clippy
              git
              nodejs_22
              rust-analyzer
              rustc
              rustfmt
              uv
            ];

            env.LIBGL_ALWAYS_SOFTWARE = "1";
            env.PLAYWRIGHT_BROWSERS_PATH = "${pkgs.playwright-driver.browsers}";
            env.PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD = "1";
          };
        }
        // lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
          # The lifecycle tools backend/justfile drives the hypervisor with. Kept
          # out of the default shell because that one is also entered on darwin,
          # where nixos-rebuild is not a thing anyone can use.
          backend = pkgs.mkShell {
            packages = with pkgs; [
              jq
              just
              nixos-rebuild
              openssh
              opentofu
              qemu-utils
            ];
          };
        }
      );

      formatter = eachSystem (pkgs: pkgs.nixfmt);

      nixosModules.runner =
        {
          config,
          lib,
          pkgs,
          ...
        }:
        let
          cfg = config.services.uibox-runner;
          inherit (lib)
            mkDefault
            mkIf
            mkOption
            mkEnableOption
            types
            ;

          graphicalEnvironment = {
            LIBGL_ALWAYS_SOFTWARE = "1";
            PLAYWRIGHT_BROWSERS_PATH = "${cfg.browsers}";
            PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD = "1";
          };
        in
        {
          options.services.uibox-runner = {
            enable = mkEnableOption "the ui-box graphical test runner" // {
              default = true;
            };

            user = mkOption {
              type = types.str;
              default = "fredrir";
              description = "Account that ui-box runs as over the ssh backend.";
            };

            group = mkOption {
              type = types.str;
              default = cfg.user;
              defaultText = lib.literalExpression "config.services.uibox-runner.user";
              description = "Group owning the golden store.";
            };

            display = mkOption {
              type = types.str;
              default = ":99";
              description = "X display that Xvfb serves and tests connect to.";
            };

            geometry = mkOption {
              type = types.str;
              default = "1280x800x24";
              description = "Xvfb screen geometry, also exported as UIBOX_DISPLAY.";
            };

            backend = mkOption {
              type = types.str;
              default = "local://";
              description = "UIBOX_BACKEND default for shells on this host.";
            };

            stateDir = mkOption {
              type = types.path;
              default = "/var/lib/ui-box-state/ui-box";
              description = "UIBOX_HOME: where ui-box reads its global .env.";
            };

            goldens = mkOption {
              type = types.path;
              default = "${cfg.stateDir}/goldens.git";
              defaultText = lib.literalExpression ''"''${config.services.uibox-runner.stateDir}/goldens.git"'';
              description = "Bare git repository holding approved screenshots.";
            };

            browsers = mkOption {
              type = types.package;
              default = pkgs.playwright-driver.browsers;
              defaultText = lib.literalExpression "pkgs.playwright-driver.browsers";
              description = ''
                Browser set the DOM driver runs. Must match the Playwright
                version ui-box-dom was built against.
              '';
            };

            tauriDriver = mkOption {
              type = types.package;
              default = self.packages.${pkgs.stdenv.hostPlatform.system}.tauri-driver;
              defaultText = lib.literalExpression "tauri-driver from the ui-box flake";
              description = ''
                WebDriver bridge the native driver runs. Its major version
                must match the Tauri major of the app under test.
              '';
            };

            packages = mkOption {
              type = types.listOf types.package;
              default = with self.packages.${pkgs.stdenv.hostPlatform.system}; [
                ui-box
                ui-box-dom
                uibox-vision
              ];
              defaultText = lib.literalExpression "ui-box, ui-box-dom and uibox-vision from the ui-box flake";
              description = "ui-box components installed system-wide.";
            };
          };

          config = mkIf cfg.enable {
            environment.systemPackages = cfg.packages ++ [
              cfg.tauriDriver
              pkgs.xorg-server
              pkgs.xdpyinfo
              pkgs.xset
              pkgs.matchbox
              pkgs.webkitgtk_4_1
            ];

            environment.variables = lib.mapAttrs (_: mkDefault) (
              graphicalEnvironment
              // {
                DISPLAY = cfg.display;
                UIBOX_BACKEND = cfg.backend;
                UIBOX_DISPLAY = cfg.geometry;
                UIBOX_GOLDENS = toString cfg.goldens;
                UIBOX_HOME = toString cfg.stateDir;
                UIBOX_NATIVE_DRIVER = lib.getExe pkgs.webkitgtk_4_1;
                UIBOX_TAURI_DRIVER = lib.getExe cfg.tauriDriver;
              }
            );

            services.openssh.settings.SetEnv = "DISPLAY=${cfg.display}";

            systemd.tmpfiles.rules = [ "d /tmp/.X11-unix 1777 root root -" ];

            systemd.services.uibox-xvfb = {
              description = "Xvfb display ${cfg.display} for ui-box";
              wantedBy = [ "multi-user.target" ];

              path = [
                pkgs.xorg-server
                pkgs.xdpyinfo
              ];

              environment = graphicalEnvironment;

              serviceConfig = {
                ExecStart = "${pkgs.xorg-server}/bin/Xvfb ${cfg.display} -screen 0 ${cfg.geometry} -nolisten tcp -noreset";
                Restart = "always";
                RestartSec = 1;
                User = cfg.user;
                Group = cfg.group;
              };

              postStart = ''
                for _ in $(seq 1 50); do
                  if xdpyinfo -display ${cfg.display} >/dev/null 2>&1; then
                    exit 0
                  fi
                  sleep 0.2
                done
                exit 1
              '';
            };

            systemd.services.uibox-wm = {
              description = "matchbox window manager on ${cfg.display}";
              wantedBy = [ "multi-user.target" ];
              after = [ "uibox-xvfb.service" ];
              requires = [ "uibox-xvfb.service" ];

              environment = graphicalEnvironment // {
                DISPLAY = cfg.display;
              };

              serviceConfig = {
                ExecStart = "${pkgs.matchbox}/bin/matchbox-window-manager -use_titlebar no";
                Restart = "always";
                RestartSec = 1;
                User = cfg.user;
                Group = cfg.group;
              };
            };

            systemd.services.uibox-goldens = {
              description = "Initialise the ui-box golden screenshot store";
              wantedBy = [ "multi-user.target" ];

              unitConfig.RequiresMountsFor = toString cfg.goldens;

              path = with pkgs; [
                coreutils
                git
              ];

              serviceConfig = {
                Type = "oneshot";
                RemainAfterExit = true;
              };

              script = ''
                goldens=${lib.escapeShellArg (toString cfg.goldens)}

                install -d -m 0755 -o ${cfg.user} -g ${cfg.group} "$(dirname "$goldens")"

                if [ ! -d "$goldens" ]; then
                  git init --bare --initial-branch=main "$goldens"
                  chown -R ${cfg.user}:${cfg.group} "$goldens"
                fi
              '';
            };
          };
        };
    };
}
