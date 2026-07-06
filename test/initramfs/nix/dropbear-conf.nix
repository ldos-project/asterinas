{ lib, pkgs, stdenvNoCC, authorized_keys ? "" }:
let
  start_dropbear_sh = builtins.toFile "start_dropbear.sh" ''
# Launcher script for dropbear

# Generate host keys
mkdir -p /etc/dropbear
dropbearkey -t rsa -f /etc/dropbear/dropbear_rsa_host_key
dropbearkey -t ecdsa -f /etc/dropbear/dropbear_ecdsa_host_key
dropbearkey -t ed25519 -f /etc/dropbear/dropbear_ed25519_host_key

# Start server with passwork logins disabled
dropbear -s -p 10.0.2.15:22
'';
  authorized_keys_file = builtins.toFile "authorized_keys" authorized_keys;
in stdenvNoCC.mkDerivation {
  name = "dropbear-conf";
  buildCommand = ''
    # Insert the provided user keys into the image
    mkdir -p $out/.ssh
    cp ${authorized_keys_file} $out/.ssh/authorized_keys
    chmod 700 $out/.ssh
    chmod 600 $out/.ssh/authorized_keys

    mkdir -p $out/bin
    cp ${start_dropbear_sh} $out/bin/start_dropbear.sh
    chmod a+x $out/bin/start_dropbear.sh
    '';
}