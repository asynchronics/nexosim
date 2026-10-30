{
  description = "Dev shell for NeXosim with Verus verification support";

  inputs = {
    nixpkgs.url = "nixpkgs";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };

        # ------------------------------------------------------------------
        # Z3 4.16.0 — download the prebuilt Linux binary and patchelf it
        # so it links against Nix glibc & libstdc++.
        # ------------------------------------------------------------------
        z3_4_16 = pkgs.stdenv.mkDerivation rec {
          pname = "z3";
          version = "4.16.0";

          src = pkgs.fetchurl {
            url = "https://github.com/Z3Prover/z3/releases/download/z3-${version}/z3-${version}-x64-glibc-2.39.zip";
            sha256 = "sha256-cojEmlvW26/XsLDR9llWuRZy2iSwjwkkKRmvFZvjQY4=";
          };

          nativeBuildInputs = [ pkgs.unzip pkgs.patchelf ];
          buildInputs = [ pkgs.glibc pkgs.gcc.cc.lib ];

          dontUnpack = true;
          dontBuild = true;

          installPhase = ''
            mkdir -p $out
            unzip -q $src -d $out
            # The archive extracts into a single top-level directory.
            mv $out/z3-${version}-x64-glibc-2.39/* $out/
            rmdir $out/z3-${version}-x64-glibc-2.39 || true

            # Patch the z3 binary
            patchelf --set-interpreter "${pkgs.glibc}/lib/ld-linux-x86-64.so.2" \
                     --set-rpath "${pkgs.glibc}/lib:${pkgs.gcc.cc.lib}/lib" \
                     $out/bin/z3

            # Also patch the shared library if present
            if [ -f "$out/bin/libz3.so" ]; then
              patchelf --set-rpath "${pkgs.glibc}/lib:${pkgs.gcc.cc.lib}/lib" \
                       $out/bin/libz3.so
            fi
          '';
        };

        # ------------------------------------------------------------------
        # Verus — download the prebuilt x86_64-linux release and patch
        # its interpreter / RPATH so it uses Nix glibc & libstdc++.
        # ------------------------------------------------------------------
        verus = pkgs.stdenv.mkDerivation rec {
          pname = "verus";
          version = "0.2026.09.20.aef82ed";

          src = pkgs.fetchurl {
            url = "https://github.com/verus-lang/verus/releases/download/release/${version}/verus-${version}-x86-linux.zip";
            sha256 = "sha256-e4cPoSvFiQFcL6tgqLPZ8Hx7Gts0ROsPrf/L9/BEezM=";
          };

          nativeBuildInputs = [ pkgs.unzip pkgs.patchelf ];
          buildInputs = [ pkgs.glibc pkgs.gcc.cc.lib ];

          dontUnpack = true;
          dontBuild = true;

          installPhase = ''
            mkdir -p $out
            unzip -q $src -d $out
            # Flatten the single top-level directory that the archive creates.
            mv $out/verus-x86-linux/* $out/
            rmdir $out/verus-x86-linux || true

            # Patch every ELF executable so it links against Nix libraries.
            for bin in $out/verus $out/cargo-verus $out/air $out/verusdoc $out/deps/verus-* $out/z3; do
              if [ -f "$bin" ] && [ -x "$bin" ]; then
                patchelf --set-interpreter "${pkgs.glibc}/lib/ld-linux-x86-64.so.2" \
                         --set-rpath "${pkgs.glibc}/lib:${pkgs.gcc.cc.lib}/lib" \
                         "$bin" || true
              fi
            done

            # Symlink the main binaries into $out/bin so nix adds them to PATH.
            mkdir -p $out/bin
            ln -s $out/verus $out/bin/verus
            ln -s $out/cargo-verus $out/bin/cargo-verus
            ln -s $out/z3 $out/bin/z3
          '';
        };
      in
      {
        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            # Rust toolchain (stable + rustup to install the exact version Verus needs)
            rustup
            rustc
            cargo
            rustfmt
            clippy

            # Verus + Z3
            verus
            z3_4_16

            # Build tools
            cmake
            pkg-config
            openssl
            patchelf

            # Misc
            git
            gnumake
            gcc
          ];

          shellHook = ''
            echo "=== NeXosim dev shell with Verus ==="
            echo "Verus: $(verus --version 2>/dev/null || echo 'not in PATH')"
            echo "Z3:    $(z3 --version 2>/dev/null || echo 'not in PATH')"
            echo "Rust:  $(rustc --version)"
            echo ""
            echo "To verify the nexosim crate:"
            echo "  cd nexosim && cargo-verus verify --features verus"
            echo ""
            echo "Normal cargo build (no Verus overhead):"
            echo "  cargo check && cargo test"
          '';
        };

        packages.verus = verus;
        packages.z3_4_16 = z3_4_16;
      });
}
