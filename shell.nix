{
  pkgs ? import <nixpkgs> { },
}:

pkgs.callPackage (
  { mkShell }:
  mkShell {
    nativeBuildInputs =
      with pkgs;
      [
        # Dolby Vision Profile 7 conversion and its end-to-end check (tests/README.md)
        dovi-tool
      ]
      ++ lib.optionals stdenv.hostPlatform.isLinux [
        lld
        cargo-xwin
      ];

    # shellHook = ''
    #   MARKER_FILE=".xwin/sdk/lib/ucrt/x86_64/ucrt.lib"
    #   if [ ! -f "$MARKER_FILE" ]; then
    #     xwin --accept-license splat --output .xwin
    #   fi
    # '';
  }
) { }
