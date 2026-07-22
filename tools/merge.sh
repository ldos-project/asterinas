#!/bin/bash

# This script replaces all ldosproject/asterinas image names with upstream names
# and commits it to simplify the git merge process
# Remember to revert the commit after resolving merge conflicts!

# usage:
# ./tools/merge.sh
# git merge upstream/main
# <fix merge conflicts>
# <remove dummy commit>
# push? I think should be good?

ast_tag="ast_tag"
ldos_tag="ldos_tag"
saw_ast_tag="false"
saw_ldos_tag="false"

git merge upstream/main
git diff > diff.txt

# find image name conflicts
while IFS= read -r line; do
    if [[ $line == "diff --cc"* ]]; then
        fname=$(echo "$line" | awk '{print $3}')
    elif [[ $line == "@@@"* ]]; then
        hunk_start=$(echo "$line" | awk '{print $2}' | grep -o '[0-9]\+' | head -1)
        hunk_len=$(echo "$line" | awk '{print $2}' | grep -o '[0-9]\+' | tail -1)
    elif [[ $line == *"ldosproject/asterinas:"* ]]; then
        saw_ldos_tag="true"
        ldos_tag=$(echo "$line" | sed -E 's^[[:space:]]*\+[[:space:]]*^^')
    elif [[ $saw_ldos_tag == "true" && $line == *"asterinas/asterinas:"* ]]; then
        saw_ast_tag="true"
        ast_tag=$(echo "$line" | sed -E 's^[[:space:]]*\+[[:space:]]*^^')
    fi
    if [[ $saw_ldos_tag == "true" && $saw_ast_tag == "true" ]]; then
        echo "${fname}@${hunk_start}@${hunk_len}@${ldos_tag}@${ast_tag}" >> docker_image_merge_conflicts.txt
        saw_ldos_tag="false"
        saw_ast_tag="false"
    fi
done < diff.txt

rm diff.txt

git merge --abort

# find original lines for image name conflicts
# and replace with asterinas/asterinas image name
while IFS= read -r line; do
    IFS="@" read -ra array <<< "$line"
    fname="${array[0]}"
    hunk_start="${array[1]}"
    hunk_len="${array[2]}"
    ldos_tag="${array[3]}"

    line_offset=$(tail -n +$hunk_start $fname | head -n $hunk_len | grep -nF -- "${ldos_tag}" | cut -d: -f1)
    line_num=$(( $line_offset - 1 + $hunk_start ))

    ldos_tag_clean=$(printf '%s\n' "${array[3]}" | sed -e 's/[]\/$*.^[]/\\&/g')
    ast_tag_clean=$(printf '%s\n' "${array[4]}" | sed -e 's/[]\/$*.^[]/\\&/g')
    echo "sed -i '' \"${line_num}s^${ldos_tag_clean}^${ast_tag_clean}^g\" ${fname}"

    # this is because I am a maccer atm
    if [[ $(uname) == "Darwin" ]]; then
        sed -i '' "${line_num}s^${ldos_tag_clean}^${ast_tag_clean}^g" ${fname}
    else
        sed -i "${line_num}s^${ldos_tag_clean}^${ast_tag_clean}^g" ${fname}
    fi
done < "docker_image_merge_conflicts.txt"

while IFS= read -r line; do
    IFS="@" read -ra array <<< "$line"
    fname="${array[0]}"
    git add $fname
done < "docker_image_merge_conflicts.txt"

git commit -m "merge.sh: reset image names; revert this commit after merge"

rm docker_image_merge_conflicts.txt
