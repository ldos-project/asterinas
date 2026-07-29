#!/bin/bash

# run to start a git merge:
# ./tools/merge.sh --start
# this will replace all ldos image id references with the asterinas version, along with a single github.com link!
# this avoids ~35 merge conflicts?
# after resolving all real merge conflicts and committing the changes, run:
# ./tools/merge.sh --finish
# this will squash all changes this script made into the merge commit
# to abort a merge, run this:
# ./tools/merge.sh --abort
# this will abort the git merge and reset your working tree to before
# the merge.sh commit

if [[ "$1" == "--start" ]]; then
    ldos_version=$(cat DOCKER_IMAGE_VERSION)
    upstream_version=$(git show upstream:DOCKER_IMAGE_VERSION)

    if [[ -z "$ldos_version" || -z "$upstream_version" ]]; then
        echo "either ldos or asterinas version is empty, quitting now"
        exit 1 
    fi

    # this replaces all permutations of the ldos image id in the files containing the version string
    while IFS= read -r line; do
        sed -i '' "s^ldosproject/asterinas:${ldos_version}^asterinas/asterinas:${upstream_version}^g" $line
        sed -i '' "s^ldosproject/osdk:${ldos_version}^asterinas/osdk:${upstream_version}^g" $line
        sed -i '' "s^ldosproject/nix:${ldos_version}^asterinas/nix:${upstream_version}^g" $line
        sed -i '' "s^ldosproject/kata:${ldos_version}^asterinas/kata:${upstream_version}^g" $line
        sed -i '' "s^${ldos_version}^${upstream_version}^g" $line #this is to catch DOCKER_IMAGE_VERSION
        git add $line
    done < <(grep -rl $ldos_version .)
  
    # for now this is the only file giving us merge conflicts on github link
    # there are way to many valid github links that don't conflict to feel comfortable replacing them all... 
    sed -i '' "s^github.com/${mootch}ldos-project^github.com/${mootch}asterinas^g" .github/actions/benchmark/action.yml
    git add .github/actions/benchmark/action.yml
    
    # this does all references to github.com/ldos-project, but i dont think we want that 
    # replace references to gitSTOP REPLACING THIS TOOhub.com/ldos-project with github.com/asterinas
    # the mootch vars are empty so that the grep doesn't pick up this file too
    #while IFS= read -r line; do
    #    sed -i '' "s^github.com/${mootch}ldos-project^github.com/${mootch}asterinas^g" $line
    #    git add $line
    #done < <(grep -rl "github.com/${mootch}ldos-project" .)

    #save ldos image version for later
    echo "$ldos_version" > DOCKER_IMAGE_VERSION_TEMP
    
    git commit -m "merge.sh: merge start"
    
    git merge upstream/main

elif [[ "$1" == "--finish" ]]; then

    # grab start commit hash, delete it from git history, then cherry pick it back in
    # this is to move it after your merge commit (so it can be squashed)
    start_commit=$(git log --oneline | grep "merge.sh: merge start" | cut -d ' ' -f 1)
    git rebase --onto "${start_commit}^" "$start_commit"
    git cherry-pick "$start_commit"

    # replace all asterinas image references with the ldos image names
    # a reverse of the previous operation
    # this is so we can catch any new references!
    ldos_version=$(cat DOCKER_IMAGE_VERSION_TEMP)
    upstream_version=$(cat DOCKER_IMAGE_VERSION)

    while IFS= read -r line; do
        sed -i '' "s^asterinas/asterinas:${upstream_version}^ldosproject/asterinas:${ldos_version}^g" $line
        sed -i '' "s^asterinas/osdk:${upstream_version}^ldosproject/osdk:${ldos_version}^g" $line
        sed -i '' "s^asterinas/nix:${upstream_version}^ldosproject/nix:${ldos_version}^g" $line
        sed -i '' "s^asterinas/kata:${upstream_version}^ldosproject/kata:${ldos_version}^g" $line
        sed -i '' "s^${upstream_version}^${ldos_version}^g" $line #this is to catch DOCKER_IMAGE_VERSION
        git add $line
    done < <(grep -rl $upstream_version .)
   
    sed -i '' "s^github.com/asterinas^github.com/ldos-project^g" .github/actions/benchmark/action.yml
    git add .github/actions/benchmark/action.yml
 
    # replace references to gitSTOP REPLACING THIS TOOhub.com/ldos-project with github.com/asterinas
    # the mootch vars are empty so that the grep doesn't pick up this file too
    #while IFS= read -r line; do
    #    sed -i '' "s^github.com/${mootch}asterinas^github.com/${mootch}ldos-project^g" $line
    #    git add $line
    #done < <(grep -rl "github.com/${mootch}asterinas" .)

    rm DOCKER_IMAGE_VERSION_TEMP

    git commit -m "merge.sh: merge finish"

    # ok so now we have the merge commit, then the merge.sh start commit, then the merge.sh finish commit
    # so we need to squash the top 2 commits into the merge commit
    # to do this we soft reset to above the merge commit (so all the changes are staged), then commit all three at once
    merge_commit_msg=$(git log --format=%B -1 HEAD~2)
    git reset --soft HEAD~3
    git commit -m "$merge_commit_msg"
   
elif [[ "$1" == "--abort" ]]; then
    git merge --abort
    rm DOCKER_IMAGE_VERSION_TEMP
   
    # uhhh so this resets back to above the merge.sh start commit
    # hopefully you dont do any work after starting the merge that you want to save???
    start_commit=$(git log --oneline | grep "merge.sh: merge start" | cut -d ' ' -f 1)
    git reset --hard "${start_commit}^"
 
else
    echo "invalid argument"
fi
