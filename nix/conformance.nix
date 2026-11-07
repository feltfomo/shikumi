# the conformance checks: one NixOS VM per Nix implementation, where an untrusted user runs
# `shikumi-conformance` against that VM's daemon. the checks themselves live in the Rust harness;
# this file only starts each machine with the right Nix and runs the binary.
{
  pkgs,
  conformance,
  # extra harness arguments, such as `[ "--perturb" "no-inputs" ]` to show a drifted computation
  # fails the check.
  extraArgs ? [ ],
}:
let
  inherit (pkgs) lib;

  implementations = {
    cppnix-2-35 = {
      package = pkgs.nixVersions.nix_2_35;
      family = "CppNix";
    };
    determinate-3-24 = {
      package = import ./determinate.nix { inherit pkgs; };
      family = "Determinate Nix";
    };
    lix-2-95 = {
      package = pkgs.lixPackageSets.lix_2_95.lix;
      family = "Lix";
    };
    lix-2-96 = {
      package = pkgs.lixPackageSets.lix_2_96.lix;
      family = "Lix";
    };
  };

  vmTest =
    name:
    { package, family }:
    let
      # a script, so the harness's arguments never pass through `su -c` quoting.
      run = pkgs.writeShellScript "run-conformance" ''
        exec ${lib.getExe conformance} --work-dir "$HOME" \
          --expect ${lib.escapeShellArg "${family} ${package.version}"} \
          ${lib.escapeShellArgs extraArgs} > "$HOME/report.txt" 2> "$HOME/stderr.txt"
      '';
    in
    pkgs.testers.runNixOSTest {
      name = "conformance-${name}";
      nodes.machine = {
        nix = {
          inherit package;
          settings = {
            # the VM has no network, and a substituter it can't reach turns every build into a
            # wait for a timeout.
            substituters = lib.mkForce [ ];
            # stated rather than inherited, since the harness's claims rest on its user being
            # untrusted. the harness also asks the daemon and fails if it says otherwise.
            trusted-users = lib.mkForce [ "root" ];
          };
        };
        users.users.alice.isNormalUser = true;
      };
      testScript = ''
        machine.wait_for_unit("multi-user.target")
        machine.wait_for_unit("nix-daemon.socket")
        status, _ = machine.execute("su - alice -c ${run}")
        machine.copy_from_machine("/home/alice/report.txt")
        machine.copy_from_machine("/home/alice/stderr.txt")
        print(machine.succeed("cat /home/alice/report.txt /home/alice/stderr.txt"))
        assert status == 0, f"shikumi-conformance exited {status}"
      '';
    };
in
lib.mapAttrs' (
  name: value: lib.nameValuePair "conformance-${name}" (vmTest name value)
) implementations
