# Development

## Set Up Environment

### NixOS

Open a terminal and navigate to the main project folder which contains the shell.nix file which
defines a development environment with all required libraries and the two IDEs VScode and Zed
Editor.

If you have direnv installed and configured, it will interpret shell.nix when you enter the
directory. If direnv is not configured, run `nix-shell shell.nix` to get the same result. In the
created environment you can start VSCode with `code` or Zed Editor with `zeditor`. The configuration
for Zed Editor is incomplete. For a smoother out-of-the-box experience use VSCode.

You can also open VSCode or any other IDE from your desktop environment and then open a terminal
inside it and perform the same steps. The recommended extension mkhl.direnv will do this for VSCode
automatically.

## Style Guide

### Tests

* expected value should be first (left) argument to assert and actual value second (right)
* don't use the prefix `test_` for test function names. This is redundant and not required in Rust.
  Rather describe the behavior that the test asserts.