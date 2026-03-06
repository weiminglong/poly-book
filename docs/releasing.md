# Releasing

This repository does not currently publish crates to crates.io. A release here
means cutting a Git tag and GitHub release for a coherent project snapshot.

## Maintainer Checklist

1. Update [CHANGELOG.md](../CHANGELOG.md)
2. Make sure CI is green on `main`
3. Confirm README and operations docs reflect the shipped behavior
4. Create and push a tag in `vX.Y.Z` format
5. Verify the GitHub `Release` workflow publishes the release notes

## Tagging

```bash
git checkout main
git pull --ff-only
git tag v0.1.0
git push origin v0.1.0
```

The GitHub workflow at `.github/workflows/release.yml` creates the release entry
automatically for tags matching `v*.*.*`.
