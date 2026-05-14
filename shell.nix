{ pkgs ? import (builtins.fetchTarball "https://github.com/NixOS/nixpkgs/archive/nixos-unstable.tar.gz") {} }:

pkgs.mkShell {
  packages = with pkgs; [
    bashInteractive
    cacert
    clang
    cmake
    curl
    fish
    git
    gnumake
    lld
    ninja
    pkg-config
    python3
    rustup
  ];

  shellHook = ''
    export CARGO_HOME="''${CARGO_HOME:-$HOME/.cargo}"
    export RUSTUP_HOME="''${RUSTUP_HOME:-$HOME/.rustup}"

    proxy_dir="$HOME/.cache/hecate-rustup-proxies"
    mkdir -p "$proxy_dir"
    unwrapped_clang="${pkgs.llvmPackages.clang-unwrapped}/bin"
    for tool in cargo rustc rustdoc rustfmt clippy-driver cargo-fmt; do
      ln -sf "${pkgs.rustup}/bin/rustup" "$proxy_dir/$tool"
    done
    ln -sf "$unwrapped_clang/clang" "$proxy_dir/clang"
    ln -sf "$unwrapped_clang/clang++" "$proxy_dir/clang++"

    export PATH="$proxy_dir:$CARGO_HOME/bin:$PATH"

    if ! rustup toolchain list | grep -q '^nightly'; then
      echo "Installing nightly Rust toolchain for Hecate..."
      rustup toolchain install nightly --profile minimal
    fi

    if ! rustup component list --toolchain nightly | grep -q '^rust-src.*installed'; then
      echo "Installing rust-src for nightly..."
      rustup component add rust-src --toolchain nightly
    fi

    if ! rustup target list --installed | grep -qx 'riscv32im-unknown-none-elf'; then
      echo "Installing riscv32im-unknown-none-elf target..."
      rustup target add riscv32im-unknown-none-elf
    fi

    echo "Hecate dev shell ready. Use: ./scripts/build_samples.sh or ./scripts/run_rust_sample.sh"
    exec fish -i
  '';
}
