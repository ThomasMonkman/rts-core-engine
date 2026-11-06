#!/bin/sh
# Builds faf-lua-debug.vsix (a VSIX is a zip: a manifest plus the extension's
# files under extension/) without needing vsce or a download, and installs
# it into VS Code. Re-run after changing the extension. See
# docs/guides/debugging-lua.md.
set -e
here="$(cd "$(dirname "$0")" && pwd)"
out="${1:-$here/../../target/faf-lua-debug.vsix}"
version="$(sed -n 's/.*"version": *"\([^"]*\)".*/\1/p' "$here/package.json" | head -1)"
work="$(mktemp -d)"
mkdir -p "$work/extension"
cp "$here/package.json" "$here/extension.js" "$work/extension/"
cat > "$work/[Content_Types].xml" <<'XML'
<?xml version="1.0" encoding="utf-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension=".json" ContentType="application/json"/><Default Extension=".js" ContentType="application/javascript"/><Default Extension=".vsixmanifest" ContentType="text/xml"/></Types>
XML
cat > "$work/extension.vsixmanifest" <<XML
<?xml version="1.0" encoding="utf-8"?>
<PackageManifest Version="2.0.0" xmlns="http://schemas.microsoft.com/developer/vsx-schema/2011" xmlns:d="http://schemas.microsoft.com/developer/vsx-schema-design/2011">
  <Metadata>
    <Identity Language="en-US" Id="faf-lua-debug" Version="$version" Publisher="rts-core-engine"/>
    <DisplayName>FAF Lua debugger (rts-core-engine)</DisplayName>
    <Description xml:space="preserve">Debug the game's Lua in rts-core-engine (--lua-debug).</Description>
    <Categories>Debuggers</Categories>
    <Properties><Property Id="Microsoft.VisualStudio.Code.Engine" Value="^1.80.0"/></Properties>
  </Metadata>
  <Installation><InstallationTarget Id="Microsoft.VisualStudio.Code"/></Installation>
  <Dependencies/>
  <Assets><Asset Type="Microsoft.VisualStudio.Code.Manifest" Path="extension/package.json" Addressable="true"/></Assets>
</PackageManifest>
XML
mkdir -p "$(dirname "$out")"
rm -f "$out"
(cd "$work" && zip -qr "$out" "[Content_Types].xml" extension.vsixmanifest extension)
rm -rf "$work"
echo "built $out"
code_cli="$(command -v code || true)"
[ -z "$code_cli" ] && [ -x "/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code" ] && code_cli="/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code"
if [ -n "$code_cli" ]; then
  "$code_cli" --install-extension "$out" --force
else
  echo "VS Code's 'code' command not found: install $out with Extensions → … → Install from VSIX…"
fi
