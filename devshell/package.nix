{
  name,
  version,
  lib,
  stdenv,
  rustPlatform,
  installShellFiles,
  darwin,
}:

rustPlatform.buildRustPackage {
  pname = name;
  inherit version;

  src = lib.cleanSource ./..;

  cargoLock = {
    lockFile = ../Cargo.lock;
  };

  buildInputs = lib.optionals stdenv.hostPlatform.isDarwin [
    darwin.apple_sdk.frameworks.Cocoa
    darwin.apple_sdk.frameworks.Security
    darwin.apple_sdk.frameworks.SystemConfiguration
  ];

  nativeBuildInputs = [
    installShellFiles
  ];

  postInstall = ''
    installShellCompletion --cmd agentbox \
      --bash <($out/bin/agentbox completions bash) \
      --fish <($out/bin/agentbox completions fish) \
      --zsh  <($out/bin/agentbox completions zsh)
  '';

  meta = with lib; {
    description = "Zero-image Podman AI sandboxes written in Rust";
    license = licenses.gpl3Only;
    platforms = platforms.linux ++ platforms.darwin;
    mainProgram = "agentbox";
  };
}
