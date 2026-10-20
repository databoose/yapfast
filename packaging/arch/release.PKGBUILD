# Packages a release's prebuilt binary from its AppImage, instead of building from source as PKGBUILD
# does: nix/appimage.nix makes that binary run on the distribution's loader and libraries, and the
# AppImage carries the dependencies' notices in usr/share/doc/yapfast. scripts/package-arch runs it,
# with the AppImage and the version; the name, description, license and dependencies are PKGBUILD's.

source "$startdir/source.PKGBUILD"
unset -f prepare build
pkgver=$YAPFAST_PKGVER
pkgrel=1
makedepends=()
# Nix stripped the binary already.
options=(!strip !debug)
source=("$YAPFAST_APPIMAGE")
sha256sums=("$YAPFAST_APPIMAGE_SHA256")

prepare() {
  # The runtime unpacks its filesystem without FUSE.
  ./"$YAPFAST_APPIMAGE" --appimage-extract >/dev/null
}

package() {
  cp -r squashfs-root/usr "$pkgdir/"
  # The AppImage's files are read-only, and older ones have notices only their owner may read.
  chmod -R u+w,a+rX "$pkgdir/usr"
}
