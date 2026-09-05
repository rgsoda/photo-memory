# AUR packaging

Two packages, because they answer different questions.

| | builds from | update when |
|---|---|---|
| `photomem-git` | whatever is on `main` | every time you rebuild |
| `photomem-bin` | the tarball attached to a `v*` tag | a release is cut |

`photomem-git` compiles a webview binding from source, which is a few minutes.
`photomem-bin` unpacks a binary. They conflict with each other on purpose.

## Publishing

The AUR is a git host with an SSH key for a login, so this needs an account —
it cannot be done for you. Once, at <https://aur.archlinux.org>: register, then
paste a public key into *My Account → SSH Public Key*.

Then, per package:

```bash
git clone ssh://aur@aur.archlinux.org/photomem-git.git
cd photomem-git
cp /path/to/photo-memory/packaging/aur/photomem-git/PKGBUILD .
makepkg --printsrcinfo > .SRCINFO      # required; the AUR rejects a push without it
git add PKGBUILD .SRCINFO
git commit -m "Initial import"
git push
```

`.SRCINFO` is generated rather than written, and it has to be regenerated
whenever the PKGBUILD changes — `pkgver` included. A push without it is
rejected by the server hook, which is the most common way a first import fails.

## Updating photomem-bin after a release

`sha256sums` in that PKGBUILD is `SKIP` until a release exists. After tagging:

```bash
cd packaging/aur/photomem-bin
updpkgsums                              # rewrites sha256sums from the real tarball
makepkg --printsrcinfo > .SRCINFO
```

Leaving it as `SKIP` works but checks nothing, which rather defeats shipping a
checksum in the first place.

## Testing before publishing

```bash
cd packaging/aur/photomem-git
makepkg -f                              # builds and runs the test suite
namcap PKGBUILD *.pkg.tar.zst           # if namcap is installed
```

`makepkg -si` installs the result. To install straight from a local PKGBUILD
without the AUR at all, `yay -B .` does the same thing through yay.
