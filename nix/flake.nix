{
  description = "Talk native desktop development environment";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";

  outputs = { nixpkgs, ... }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };
      webrtcArchive = pkgs.fetchurl {
        url = "https://github.com/livekit/rust-sdks/releases/download/webrtc-51ef663/webrtc-linux-x64-release.zip";
        hash = "sha256-3OnZQUzY4syaRxwFRIGFGyV60qh5ESzlvlY2m0/v2u0=";
      };
      webrtc = pkgs.runCommand "talk-libwebrtc-51ef663" {
        nativeBuildInputs = [ pkgs.unzip ];
      } ''
        unzip -q ${webrtcArchive} -d unpacked
        mv unpacked/linux-x64-release "$out"
      '';
      windowsWebrtcArchive = pkgs.fetchurl {
        url = "https://github.com/livekit/rust-sdks/releases/download/webrtc-51ef663/webrtc-win-x64-release.zip";
        hash = "sha256-ClalyRs7e0IiCCuKCTiNFqgCzeItZylb6yTFVwB1WzA=";
      };
      windowsWebrtc = pkgs.runCommand "talk-libwebrtc-windows-51ef663" {
        nativeBuildInputs = [ pkgs.unzip ];
      } ''
        unzip -q ${windowsWebrtcArchive} -d unpacked
        mv unpacked/win-x64-release "$out"
      '';
      windowsSdkToolsArchive = pkgs.fetchurl {
        url = "https://api.nuget.org/v3-flatcontainer/microsoft.windows.sdk.cpp/10.0.26100.8249/microsoft.windows.sdk.cpp.10.0.26100.8249.nupkg";
        hash = "sha256-+Hh7L2Z4Fkrnib3KYkfmlsKg9SmjnOuWnW7z1pqYcTE=";
      };
      windowsShaderCompiler = pkgs.runCommand "talk-fxc-10.0.26100.8249" {
        nativeBuildInputs = [ pkgs.unzip ];
      } ''
        mkdir -p "$out"
        unzip -j ${windowsSdkToolsArchive} \
          'c/bin/10.0.26100.0/x64/fxc.exe' \
          'c/bin/10.0.26100.0/x64/d3dcompiler_47.dll' -d "$out"
      '';
      libraries = with pkgs; [
        alsa-lib libpulseaudio fontconfig freetype glib libxkbcommon openssl
        wayland vulkan-loader libGL
        xorg.libX11 xorg.libXcursor xorg.libXi xorg.libXrandr
        xorg.libxcb
      ];
    in {
      devShells.${system} = {
        default = pkgs.mkShell {
          nativeBuildInputs = with pkgs; [
            rustup pkg-config cmake ninja llvmPackages_21.clang
            protobuf curl jq docker-compose python3
          ];
          buildInputs = libraries;
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath libraries;
          CC = "clang";
          CXX = "clang++";
          PROTOC = "${pkgs.protobuf}/bin/protoc";
          # webrtc-sys must not download binaries from inside a Nix build sandbox.
          LK_CUSTOM_WEBRTC = "${webrtc}";
          shellHook = ''
            export LD_LIBRARY_PATH="/run/opengl-driver/lib:$LD_LIBRARY_PATH"
            echo "Talk dev shell. Run scripts/check or cargo run --locked."
          '';
        };
        windows = pkgs.mkShell {
          nativeBuildInputs = with pkgs; [
            rustup cargo-xwin cmake ninja protobuf python3 curl jq zip wineWowPackages.stable
            llvmPackages_21.clang llvmPackages_21.lld llvmPackages_21.llvm
          ];
          CC = "clang";
          CXX = "clang++";
          PROTOC = "${pkgs.protobuf}/bin/protoc";
          LK_CUSTOM_WEBRTC = "${windowsWebrtc}";
          TALK_FXC_EXE = "${windowsShaderCompiler}/fxc.exe";
        };
      };
    };
}
