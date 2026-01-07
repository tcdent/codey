# Release Process

## 1. Bump version

```bash
# Edit Cargo.toml version
vim Cargo.toml

git add Cargo.toml Cargo.lock
git commit -m "Bump version to X.Y.Z"
git push origin main
```

## 2. Tag and push

```bash
git tag vX.Y.Z
git push origin vX.Y.Z
```

This triggers the [Release workflow](https://github.com/tcdent/codey/actions/workflows/release.yml) which builds for:
- `codey-darwin-arm64` (macOS ARM)
- `codey-linux-x86_64` (Linux x86)
- `codey-linux-arm64` (Linux ARM)

## 3. Update Homebrew tap

Once the release workflow completes, get SHA256 hashes:

```bash
VERSION=X.Y.Z
curl -sL https://github.com/tcdent/codey/releases/download/v$VERSION/codey-darwin-arm64.tar.gz | shasum -a 256
curl -sL https://github.com/tcdent/codey/releases/download/v$VERSION/codey-linux-x86_64.tar.gz | shasum -a 256
curl -sL https://github.com/tcdent/codey/releases/download/v$VERSION/codey-linux-arm64.tar.gz | shasum -a 256
```

Update `~/Work/tap/Formula/codey.rb`:
- `version "X.Y.Z"`
- SHA256 hashes for each platform

```bash
cd ~/Work/tap
git add -A
git commit -m "Update codey to vX.Y.Z"
git push
```

## Version scheme

```
0.1.0-alpha.1   # Early development
0.1.0-rc.1      # Release candidate
0.1.0           # Stable release
```
