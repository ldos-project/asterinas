import argparse
import subprocess
import os


def replace_string_in_file(f, src, dst):
    with open(f, "r", encoding="utf-8") as file:
        string = file.read()

    new_string = string.replace(src, dst)

    with open(f, "w", encoding="utf-8") as file:
        file.write(new_string)


def run_cmd(cmd):
    try:
        result = subprocess.run(
            cmd, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True
        )
        return result.stdout.strip()
    except subprocess.CalledProcessError as e:
        print(e.stdout)
        print(f"Error executing {' '.join(cmd)}: {e.stderr}")
        return None


def git_add(f):
    return run_cmd(["git", "add", f])


def git_commit(msg):
    return run_cmd(["git", "commit", "-m", msg])


# finds the commit hash of the commit with the specified message
def get_commit_hash(msg):
    log_string = run_cmd(["git", "log", "--oneline"])
    log = log_string.split("\n")

    for c in log:
        if msg in c:
            return c.split(" ")[0]


def delete_file(f):
    if os.path.exists(f):
        os.remove(f)
    else:
        print(f"{f} not found")


if __name__ == "__main__":
    DESCRIPTION = """\
        Helper for merging upstream asterinas into ldos fork.
        
        It temporarily rewrites ldos-specific image references + a github link
        to their upstream equivalents to avoid common merge conflicts, then
        restores them afterwards.
    """

    EPILOG = """\
        Usage:
          1. python3 tools/merge_upstream.py --start    # rewrite refs, commit, then git merge upstream/main
          2. resolve any real merge conflicts and commit them
          3. python3 tools/merge_upstream.py --finish   # restore refs and squash into the merge commit
        
          python3 tools/merge_upstream.py --abort       # bail out and reset to before the merge
    """

    parser = argparse.ArgumentParser(
        prog="merge_upstream.py",
        description=DESCRIPTION,
        epilog=EPILOG,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument(
        "-s",
        "--start",
        action="store_true",
        help="rewrite ldos image refs to upstream, commit, and start upstream merge",
    )
    parser.add_argument(
        "-f",
        "--finish",
        action="store_true",
        help="restore ldos image refs and squash this script's changes into the merge commit",
    )
    parser.add_argument(
        "-a",
        "--abort",
        action="store_true",
        help="abort the git merge and reset the working tree to before the merge",
    )
    args = parser.parse_args()

    if args.start:
        with open("DOCKER_IMAGE_VERSION", "r", encoding="utf-8") as file:
            ldos_version = file.readline().rstrip("\n")

        upstream_version = run_cmd(["git", "show", "upstream:DOCKER_IMAGE_VERSION"])

        print(f"ldos_version: {ldos_version}")
        print(f"upstream_version: {upstream_version}")

        out = run_cmd(["grep", "-rl", ldos_version, "."])
        files = out.split("\n")

        for f in files:
            replace_string_in_file(
                f,
                f"ldosproject/asterinas:{ldos_version}",
                f"asterinas/asterinas:{upstream_version}",
            )
            replace_string_in_file(
                f,
                f"ldosproject/osdk:{ldos_version}",
                f"asterinas/osdk:{upstream_version}",
            )
            replace_string_in_file(
                f,
                f"ldosproject/nix:{ldos_version}",
                f"asterinas/nix:{upstream_version}",
            )
            replace_string_in_file(
                f,
                f"ldosproject/kata:{ldos_version}",
                f"asterinas/kata:{upstream_version}",
            )
            replace_string_in_file(f, f"{ldos_version}", f"{upstream_version}")
            git_add(f)

        replace_string_in_file(
            ".github/actions/benchmark/action.yml",
            "github.com/ldos-project",
            "github.com/asterinas",
        )
        git_add(".github/actions/benchmark/action.yml")

        with open("DOCKER_IMAGE_VERSION_TEMP", "w", encoding="utf-8") as file:
            file.write(ldos_version)
        print("Wrote DOCKER_IMAGE_VERSION_TEMP")

        out = git_commit("merge_upstream.py: merge start")
        print(out)

        # this will error if there are merge conflicts (but thats ok)
        run_cmd(["git", "merge", "upstream/main"])

    elif args.finish:
        # grab start commit hash
        commit_hash = get_commit_hash("merge_upstream.py: merge start")

        # delete start commit from git history
        out = run_cmd(["git", "rebase", "--onto", f"{commit_hash}^", commit_hash])
        print(out)

        # cherry-pick start commit back in
        out = run_cmd(["git", "cherry-pick", commit_hash])
        print(out)

        with open("DOCKER_IMAGE_VERSION_TEMP", "r", encoding="utf-8") as file:
            ldos_version = file.readline().rstrip("\n")
        with open("DOCKER_IMAGE_VERSION", "r", encoding="utf-8") as file:
            upstream_version = file.readline().rstrip("\n")

        print(f"ldos_version: {ldos_version}")
        print(f"upstream_version: {upstream_version}")

        out = run_cmd(["grep", "-rl", upstream_version, "."])
        files = out.split("\n")

        for f in files:
            replace_string_in_file(
                f,
                f"asterinas/asterinas:{upstream_version}",
                f"ldosproject/asterinas:{ldos_version}",
            )
            replace_string_in_file(
                f,
                f"asterinas/osdk:{upstream_version}",
                f"ldosproject/osdk:{ldos_version}",
            )
            replace_string_in_file(
                f,
                f"asterinas/nix:{upstream_version}",
                f"ldosproject/nix:{ldos_version}",
            )
            replace_string_in_file(
                f,
                f"asterinas/kata:{upstream_version}",
                f"ldosproject/kata:{ldos_version}",
            )
            replace_string_in_file(f, f"{upstream_version}", f"{ldos_version}")
            git_add(f)

        replace_string_in_file(
            ".github/actions/benchmark/action.yml",
            "github.com/asterinas",
            "github.com/ldos-project",
        )
        out = git_add(".github/actions/benchmark/action.yml")
        out = git_commit("merge_upstream.py: merge finish")
        print(out)

        delete_file("DOCKER_IMAGE_VERSION_TEMP")
        print("Deleted DOCKER_IMAGE_VERSION_TEMP")

        # ok so now we have the merge commit, then the merge_upstream.py start commit, then the merge_upstream.py finish commit
        # so we need to squash the top 2 commits into the merge commit
        # to do this we soft reset to above the merge commit (so all the changes are staged), then commit all three at once

        msg = run_cmd(["git", "log", "--format=%B", "-1", "HEAD~2"])
        out = run_cmd(["git", "reset", "--soft", "HEAD~3"])
        print(out)
        out = git_commit(msg)
        print(out)

    elif args.abort:
        out = run_cmd(["git", "merge", "--abort"])
        print(out)

        delete_file("DOCKER_IMAGE_VERSION_TEMP")
        print("Deleted DOCKER_IMAGE_VERSION_TEMP")

        # resets back to above the merge_upstream.py start commit
        commit_hash = get_commit_hash("merge_upstream.py: merge start")

        out = run_cmd(["git", "reset", "--hard", f"{commit_hash}^"])
        print(out)

    else:
        print("Invalid argument!")
