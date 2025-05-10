{
  pkgs ? import <nixpkgs> { config.allowUnfree = true; },
}:
let
  overrides = builtins.fromTOML (builtins.readFile ./rust-toolchain.toml);
  libPath =
    with pkgs;
    lib.makeLibraryPath [
      # load external libraries that you need in your rust project here
    ];
in
pkgs.mkShell {
  buildInputs = with pkgs; [
    # IDEs
    vscode.fhs # IDE
    zed-editor # alternative IDE

    # Rust language
    cargo
    rust-analyzer # Language server for rust language
    rustfmt
    clippy # linter for Rust

    # Nix language
    nil # Language server for nix language (Presumably better than nixd)
    nixfmt-rfc-style # Nix language formatter (executable is nixfmt)

    # Debugger
    lldb

    # for VSCode Extensions
    libxkbcommon # for slint server VSCode extension

    # Qt 6 (used as Slint backend)
    qt6.qtbase
  ];

  RUSTC_VERSION = overrides.toolchain.channel;

  # Add precompiled library to rustc search path
  RUSTFLAGS = (
    builtins.map (a: "-L ${a}/lib") [
      # add libraries here (e.g. pkgs.libvmi)
    ]
  );
  LD_LIBRARY_PATH = libPath;

  # Print log up to debug level during execution of Ravioni.
  RUST_LOG = "debug";
}
