{
  description = "Yapfast, a native Discord client";

  # Development happens in nix/flake.nix's shells; this flake only builds the
  # app, so `nix profile add github:yapfast/yapfast` needs no checkout.
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { nixpkgs, rust-overlay, ... }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; overlays = [ rust-overlay.overlays.default ]; };
    in {
      packages.${system} = rec {
        yapfast = import ./nix/package.nix { inherit pkgs; };
        default = yapfast;
      };
    };
}
