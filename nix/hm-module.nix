# Home-manager module for HanhCute speech-to-text
#
# Provides a systemd user service for autostart.
# Usage: imports = [ hanhcute.homeManagerModules.default ];
#        services.hanhcute.enable = true;
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.services.hanhcute;
in
{
  options.services.hanhcute = {
    enable = lib.mkEnableOption "HanhCute speech-to-text user service";

    package = lib.mkOption {
      type = lib.types.package;
      defaultText = lib.literalExpression "hanhcute.packages.\${system}.hanhcute";
      description = "The HanhCute package to use.";
    };
  };

  config = lib.mkIf cfg.enable {
    systemd.user.services.hanhcute = {
      Unit = {
        Description = "HanhCute speech-to-text";
        After = [ "graphical-session.target" ];
        PartOf = [ "graphical-session.target" ];
      };
      Service = {
        ExecStart = "${cfg.package}/bin/hanhcute";
        Restart = "on-failure";
        RestartSec = 5;
      };
      Install.WantedBy = [ "graphical-session.target" ];
    };
  };
}
