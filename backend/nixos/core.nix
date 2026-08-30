{ pkgs, ... }:

{
  environment.systemPackages = with pkgs; [
    curl
    fd
    gcc
    git
    gnumake
    jq
    lsof
    neovim
    pkg-config
    ripgrep
    # ui-box push/pull runs `rsync -e ssh`, which needs rsync on this end too.
    rsync
    unzip
    wget
  ];

  programs.git = {
    enable = true;

    config = {
      init.defaultBranch = "main";
      pull.rebase = true;
    };
  };

  environment.variables.EDITOR = "nvim";

  # SSH forwards the caller's TERM. Without the matching terminfo every shell
  # start falls back to a dumb terminal. Terminfo outputs only, so kilobytes.
  environment.enableAllTerminfo = true;
}
