{
  description = "Terminal pixel-art office for AI coding agents";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        inherit (pkgs) lib stdenv;
        cargoToml = lib.importTOML ./Cargo.toml;

        # winit and softbuffer dlopen these at runtime for `pixtuoid floating`,
        # so they must be on the binary's RUNPATH, not just present at build time.
        linuxRuntimeLibs = with pkgs; [
          libxkbcommon
          wayland
          libx11
          libxcursor
          libxi
          libxrandr
          libxcb
        ];

        pixtuoid = pkgs.rustPlatform.buildRustPackage {
          pname = "pixtuoid";
          inherit (cargoToml.workspace.package) version;

          src = lib.cleanSource ./.;
          cargoLock.lockFile = ./Cargo.lock;

          # Both binaries in one output: `install` finds the shim as a sibling
          # of the running exe.
          cargoBuildFlags = [
            "--package"
            "pixtuoid"
            "--package"
            "pixtuoid-hook"
          ];

          nativeBuildInputs = [ pkgs.installShellFiles ] ++ lib.optionals stdenv.hostPlatform.isLinux [ pkgs.pkg-config ];
          buildInputs = lib.optionals stdenv.hostPlatform.isLinux [ pkgs.alsa-lib ];

          # The x86_64 lld pin assumes a system lld the stdenv doesn't ship;
          # homebrew-core drops it the same way.
          postPatch = ''
            rm .cargo/config.toml
          '';

          # The suite reads HOME, spawns sockets and asserts committed goldens;
          # CI owns it.
          doCheck = false;

          postInstall = lib.optionalString (stdenv.buildPlatform.canExecute stdenv.hostPlatform) ''
            installShellCompletion --cmd pixtuoid \
              --bash <($out/bin/pixtuoid completions bash) \
              --fish <($out/bin/pixtuoid completions fish) \
              --zsh <($out/bin/pixtuoid completions zsh)
            $out/bin/pixtuoid man > pixtuoid.1
            installManPage pixtuoid.1
          '';

          postFixup = lib.optionalString stdenv.hostPlatform.isLinux ''
            patchelf --add-rpath ${lib.makeLibraryPath linuxRuntimeLibs} $out/bin/pixtuoid
          '';

          meta = {
            inherit (cargoToml.workspace.package) description homepage;
            license = lib.licenses.mit;
            mainProgram = "pixtuoid";
          };
        };
      in
      {
        packages = {
          inherit pixtuoid;
          default = pixtuoid;
        };
      }
    );
}
