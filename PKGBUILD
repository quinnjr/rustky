# Maintainer: Joseph Quinn <quinn.josephr@protonmail.com>
pkgname=rustky
pkgver=0.1.2
pkgrel=1
pkgdesc='A modern conky-like system monitor for Wayland'
arch=('x86_64')
license=('MIT')
depends=('wayland')
makedepends=('cargo' 'wayland-protocols')
optdepends=(
  'python: Python scripting support (build with python-scripting feature)'
)

# Local source tree — run makepkg from the project root
source=()
sha256sums=()

prepare() {
  cd "$startdir"
  export RUSTUP_TOOLCHAIN=nightly
  cargo fetch --target "$(rustc -vV | sed -n 's/host: //p')"
}

build() {
  cd "$startdir"
  export RUSTUP_TOOLCHAIN=nightly
  export CARGO_TARGET_DIR=target
  cargo build --release
}

package() {
  cd "$startdir"
  install -Dm755 "target/release/$pkgname" "$pkgdir/usr/bin/$pkgname"
  install -Dm644 rustky.service "$pkgdir/usr/lib/systemd/user/$pkgname.service"
  install -Dm644 examples/config.toml "$pkgdir/usr/share/doc/$pkgname/config.toml.example"
  install -Dm644 README.md "$pkgdir/usr/share/doc/$pkgname/README.md"
}
