# @polylint/polylint-win32-x64

The prebuilt `poly` binary for **Windows x64** (`x86_64-pc-windows-msvc`).

This package exists only so that [`@polylint/polylint`](https://www.npmjs.com/package/@polylint/polylint)
can pull exactly one platform binary through `optionalDependencies`. Install that
package instead — it is what puts the `poly` command on your `PATH`:

```sh
pnpm add --global @polylint/polylint
```
