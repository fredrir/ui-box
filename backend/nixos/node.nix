{ pkgs, ... }:

{
  environment.systemPackages = with pkgs; [
    biome
    corepack
    nodejs_22
    typescript
    typescript-language-server
  ];
}
