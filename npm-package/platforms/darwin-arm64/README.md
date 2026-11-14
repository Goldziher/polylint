# @polylint/polylint-darwin-arm64

The prebuilt `poly` binary for **macOS (Apple silicon)** (`aarch64-apple-darwin`).

This package exists only so that [`@polylint/polylint`](https://www.npmjs.com/package/@polylint/polylint)
can pull exactly one platform binary through `optionalDependencies`. Install that
package instead — it is what puts the `poly` command on your `PATH`:

```sh
pnpm add --global @polylint/polylint
```
