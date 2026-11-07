# the conformance checks: one NixOS VM per Nix implementation, where an untrusted user runs
# `shikumi-conformance` against that VM's daemon three times, once through each transport: the
# CLI, the daemon's socket, and `ssh-ng`. the checks themselves live in the Rust harness; this file
# only starts each machine with the right Nix and runs the binary.
#
# the `ssh-ng` run reaches the same machine, as the same untrusted user, through sshd. that proves
# the client speaks to `nix-daemon --stdio` and that trust follows the user the far side logs in
# as; it doesn't prove anything about a second store or a network between two machines.
{
  pkgs,
  conformance,
  # extra harness arguments for every run, such as `[ "--perturb" "no-inputs" ]` or
  # `[ "--remove" "readdressing" ]`, to show the checks fail when a computation drifts or a step
  # is left out.
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

  transports = [
    "cli"
    "socket"
    "ssh"
  ];

  vmTest =
    name:
    { package, family }:
    let
      # a script, so the harness's arguments never pass through `su -c` quoting. each run ends
      # with a collection over the whole store, which is why it runs only in a throwaway VM.
      run = pkgs.writeShellScript "run-conformance" ''
        transport=$1
        exec ${lib.getExe conformance} --work-dir "$HOME" \
          --transport "$transport" --ssh alice@localhost --collect-garbage \
          --expect ${lib.escapeShellArg "${family} ${package.version}"} \
          ${lib.escapeShellArgs extraArgs} \
          > "$HOME/report-$transport.txt" 2> "$HOME/stderr-$transport.txt"
      '';
      # alice logs in to this machine as herself, so the far side of `ssh-ng` is the same
      # untrusted user.
      sshSetup = pkgs.writeShellScript "ssh-setup" ''
        set -euo pipefail
        mkdir -p -m 700 "$HOME/.ssh"
        ssh-keygen -q -t ed25519 -N "" -f "$HOME/.ssh/id_ed25519"
        cp "$HOME/.ssh/id_ed25519.pub" "$HOME/.ssh/authorized_keys"
        ssh-keyscan -t ed25519 localhost > "$HOME/.ssh/known_hosts"
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
        services.openssh.enable = true;
        users.users.alice.isNormalUser = true;
        # each run ends by collecting garbage, and the test script's own programs are rooted only
        # by this.
        system.extraDependencies = [
          run
          sshSetup
        ];
      };
      testScript = ''
        transports = ${builtins.toJSON transports}
        machine.wait_for_unit("multi-user.target")
        machine.wait_for_unit("nix-daemon.socket")
        machine.wait_for_unit("sshd.service")
        machine.succeed("su - alice -c ${sshSetup}")
        statuses = {}
        for transport in transports:
            statuses[transport], _ = machine.execute(f"su - alice -c '${run} {transport}'")
            for kind in ["report", "stderr"]:
                machine.copy_from_machine(f"/home/alice/{kind}-{transport}.txt")
            print(machine.succeed(f"cat /home/alice/report-{transport}.txt /home/alice/stderr-{transport}.txt"))
        # one derivation every run builds alike: its output path and NAR hash have to be the same
        # whichever transport built it.
        shared = {
            transport: [
                line
                for line in machine.succeed(f"cat /home/alice/report-{transport}.txt").splitlines()
                if "\tshared-derivation\t" in line
            ]
            for transport in transports
        }
        assert shared["cli"] and all(lines == shared["cli"] for lines in shared.values()), shared
        assert all(status == 0 for status in statuses.values()), f"shikumi-conformance exited {statuses}"
      '';
    };
in
lib.mapAttrs' (
  name: value: lib.nameValuePair "conformance-${name}" (vmTest name value)
) implementations
