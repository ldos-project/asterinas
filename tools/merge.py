import argparse
import subprocess
import os

# run to start a git merge:
#   python3 tools/merge.py --start
# this will replace all ldos image id references with the asterinas version, along with a single github.com link!
# this avoids ~35 merge conflicts?

# after resolving all real merge conflicts and committing the changes, run:
#   python3 tools/merge.py --finish
# this will squash all changes this script made into the merge commit

# to abort a merge, run this:
#   python3 tools/merge.py --abort
# this will abort the git merge and reset your working tree to before
# the merge.py commit


def replace_string_in_file(f, src, dst):
    with open(f, "r", encoding="utf-8") as file:
        string = file.read()

    new_string = string.replace(src, dst)

    with open(f, "w", encoding="utf-8") as file:
        file.write(new_string)    

def run_cmd(cmd):
    try:
        result = subprocess.run(
            cmd, 
            check=True, 
            stdout=subprocess.PIPE, 
            stderr=subprocess.PIPE, 
            text=True
        )
        return result.stdout.strip()
    except subprocess.CalledProcessError as e:
        # i print the stdout to show the merge conflict output
        # idk how to handle it otherwise!
        print(e.stdout)
        print(f"Error executing {' '.join(cmd)}: {e.stderr}")
        return None

def git_add(f):
    return run_cmd(["git", "add", f])

def git_commit(msg):
    return run_cmd(["git", "commit", "-m", msg])

# git log --oneline | grep $msg | cut -d ' ' -f 1
def get_commit_hash(msg):
    log_string = run_cmd(["git", "log", "--oneline"])
    log = log_string.split('\n')

    for c in log:
        if msg in c:
            return c.split(' ')[0]

def delete_file(f):
    if os.path.exists(f):
            os.remove(f)
    else:
        print(f"{f} not found")

if __name__=="__main__":
    parser = argparse.ArgumentParser(prog="merge helper")
    parser.add_argument('-s', '--start', action='store_true')
    parser.add_argument('-f', '--finish', action='store_true')
    parser.add_argument('-a', '--abort', action='store_true')
    args = parser.parse_args()

    if args.start:
        with open("DOCKER_IMAGE_VERSION", "r", encoding="utf-8") as file:
            ldos_version = file.readline().rstrip('\n')

        #git show upstream:DOCKER_IMAGE_VERSION
        upstream_version = run_cmd(["git", "show", "upstream:DOCKER_IMAGE_VERSION"])

        print(f"ldos_version: {ldos_version}")
        print(f"upstream_version: {upstream_version}")

        out = run_cmd(["grep", "-rl", ldos_version, "."])
        files = out.split('\n')

        for f in files:
            replace_string_in_file(f, f"ldosproject/asterinas:{ldos_version}", f"asterinas/asterinas:{upstream_version}")
            replace_string_in_file(f, f"ldosproject/osdk:{ldos_version}", f"asterinas/osdk:{upstream_version}")
            replace_string_in_file(f, f"ldosproject/nix:{ldos_version}", f"asterinas/nix:{upstream_version}")
            replace_string_in_file(f, f"ldosproject/kata:{ldos_version}", f"asterinas/kata:{upstream_version}")
            replace_string_in_file(f, f"{ldos_version}", f"{upstream_version}")
            git_add(f)

        replace_string_in_file(".github/actions/benchmark/action.yml", "github.com/ldos-project", "github.com/asterinas")
        git_add(".github/actions/benchmark/action.yml")

        with open("DOCKER_IMAGE_VERSION_TEMP", "w", encoding="utf-8") as file:
            file.write(ldos_version)
        print("Wrote DOCKER_IMAGE_VERSION_TEMP")
        
        # git commit -m "merge.py: merge start"
        out = git_commit("merge.py: merge start")
        print(out)

        # git merge upstream/main
        # this will error if there are merge conflicts (but thats ok)
        run_cmd(["git", "merge", "upstream/main"])

    elif args.finish:
        # grab start commit hash, delete it from git history, then cherry pick it back in
        # this is to move it after your merge commit (so it can be squashed)

        commit_hash = get_commit_hash("merge.py: merge start")
        
        # git rebase --onto "${commit_hash}^" "$commit_hash"
        out = run_cmd(["git", "rebase", "--onto", f"{commit_hash}^", commit_hash])
        print(out)

        # git cherry-pick "$commit_hash"
        out = run_cmd(["git", "cherry-pick", commit_hash])
        print(out)

        with open("DOCKER_IMAGE_VERSION_TEMP", "r", encoding="utf-8") as file:
            ldos_version = file.readline().rstrip('\n')
        with open("DOCKER_IMAGE_VERSION", "r", encoding="utf-8") as file:
            upstream_version = file.readline().rstrip('\n')

        print(f"ldos_version: {ldos_version}")
        print(f"upstream_version: {upstream_version}")

        out = run_cmd(["grep", "-rl", upstream_version, "."])
        files = out.split('\n')

        for f in files:
            replace_string_in_file(f, f"asterinas/asterinas:{upstream_version}", f"ldosproject/asterinas:{ldos_version}")
            replace_string_in_file(f, f"asterinas/osdk:{upstream_version}", f"ldosproject/osdk:{ldos_version}")
            replace_string_in_file(f, f"asterinas/nix:{upstream_version}", f"ldosproject/nix:{ldos_version}")
            replace_string_in_file(f, f"asterinas/kata:{upstream_version}", f"ldosproject/kata:{ldos_version}")
            replace_string_in_file(f, f"{upstream_version}", f"{ldos_version}")
            git_add(f)

        replace_string_in_file(".github/actions/benchmark/action.yml", "github.com/asterinas", "github.com/ldos-project")
        out = git_add(".github/actions/benchmark/action.yml")
        out = git_commit("merge.py: merge finish")
        print(out)

        delete_file("DOCKER_IMAGE_VERSION_TEMP")
        print("Deleted DOCKER_IMAGE_VERSION_TEMP")

        # ok so now we have the merge commit, then the merge.py start commit, then the merge.py finish commit
        # so we need to squash the top 2 commits into the merge commit
        # to do this we soft reset to above the merge commit (so all the changes are staged), then commit all three at once
        
        # we can do this like this:
        #merge_commit_msg=$(git log --format=%B -1 HEAD~2)
        #git reset --soft HEAD~3
        #git commit -m "$merge_commit_msg"

        msg = run_cmd(["git", "log", "--format=%B", "-1", "HEAD~2"])
        out = run_cmd(["git", "reset", "--soft", "HEAD~3"])
        print(out)
        out = git_commit(msg)
        print(out)

    elif args.abort:
        # git merge --abort
        out = run_cmd(["git", "merge", "--abort"])
        print(out)

        delete_file("DOCKER_IMAGE_VERSION_TEMP")
        print("Deleted DOCKER_IMAGE_VERSION_TEMP")

        # # uhhh so this resets back to above the merge.py start commit
        # # hopefully you dont do any work after starting the merge that you want to save???
        commit_hash = get_commit_hash("merge.py: merge start")        

        # git reset --hard "${commit_hash}^"
        out = run_cmd(["git", "reset", "--hard", f"{commit_hash}^"])
        print(out)
  
    else:
        print("Invalid argument!")


    
