{
  name,
  version,
  lib,
  rustPlatform,
  installShellFiles,
  completions ? null,
}:

rustPlatform.buildRustPackage {
  pname = name;
  inherit version;

  src = lib.cleanSource ./..;

  cargoLock = {
    lockFile = ../Cargo.lock;
  };

  nativeBuildInputs = [
    installShellFiles
  ];

  doCheck = false;

  postInstall =
    if completions != null then
      ''
        mkdir -p $out/share
        cp -r ${completions}/share/* $out/share/
      ''
    else
      ''
        installShellCompletion --cmd agentbox \
          --bash <($out/bin/agentbox completions bash) \
          --fish <($out/bin/agentbox completions fish) \
          --zsh  <($out/bin/agentbox completions zsh)
      '';

  meta = with lib; {
    description = "Zero-image Podman AI sandboxes written in Rust (statically linked)";
    license = licenses.gpl3Only;
    platforms = platforms.linux;
    mainProgram = "agentbox";
  };
}
