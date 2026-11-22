{
  name,
  version,
  dockerTools,
  agentbox,
  buildEnv,
  ...
}:

dockerTools.buildImage {
  inherit name;
  tag = "v${version}";

  copyToRoot = buildEnv {
    name = "image-root";
    paths = [ agentbox ];
    pathsToLink = [ "/bin" ];
  };

  config = {
    Entrypoint = [ "${agentbox}/bin/agentbox" ];
  };
}
