{ pkgs, agentbox }:

pkgs.runCommand "agentbox-completions"
  {
    nativeBuildInputs = [ pkgs.installShellFiles ];
  }

  ''
    installShellCompletion --cmd agentbox \
      --bash <(${agentbox}/bin/agentbox completions bash) \
      --fish <(${agentbox}/bin/agentbox completions fish) \
      --zsh  <(${agentbox}/bin/agentbox completions zsh)
  ''
