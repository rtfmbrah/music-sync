{
  description = "Reproducible development environment for music-sync v2";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, rust-overlay }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in {
      devShells = forAllSystems (system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ (import rust-overlay) ];
          };
          rustToolchain = pkgs.rust-bin.stable."1.95.0".minimal.override {
            extensions = [ "clippy" "rustfmt" ];
            targets = [ "x86_64-unknown-linux-musl" ];
          };
          muslCompiler = pkgs.pkgsCross.musl64.stdenv.cc;
        in {
          default = pkgs.mkShell {
            packages = with pkgs; [
              rustToolchain
              just
              sqlite
              pkg-config
              yt-dlp
              ffmpeg-headless
              chromaprint
              python3
              cargo-audit
              muslCompiler
            ];
            CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER = "${muslCompiler}/bin/x86_64-unknown-linux-musl-gcc";
          };
        });
    };
}
