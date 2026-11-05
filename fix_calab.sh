#!/bin/sh
cd "/f/AI/My_Projects/AI_Project/FileManager" || exit 1
sha=df94d5013a408ad648bd92535cf741b6b02573bc
raw=$(git cat-file commit "$sha")
subject=$(printf '%s' "$raw" | awk 'f{print; exit} /^$/{f=1}')
esc() { printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' | tr -d '\r\n'; }
printf '%s' "$subject" > .git/t_raw.txt
esc "$subject" > .git/t_esc.txt
printf '%s' "{\"content\":\"FileManager: $(esc "$subject")\"}" > .git/t_json.txt
echo "---raw---"; od -A x -t x1z .git/t_raw.txt | head -2
echo "---esc---"; od -A x -t x1z .git/t_esc.txt | head -2
echo "---json---"; od -A x -t x1z .git/t_json.txt | head -3
