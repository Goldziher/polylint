# @polylint/polylint-linux-x64-gnu

The prebuilt `poly` binary for **Linux x64 (glibc)** (`x86_64-unknown-linux-gnu`).

This package exists only so that [`@polylint/polylint`](https://www.npmjs.com/package/@polylint/polylint)
can pull exactly one platform binary through `optionalDependencies`. Install that
package instead — it is what puts the `poly` command on your `PATH`:

```sh
pnpm add --global @polylint/polylint
```
