#!/bin/sh
# Builds the JVM proxy agent jar. Needs javac and jar (JDK 9 or newer) on PATH
# or under JAVA_HOME.
#
#   java-proxy-agent/build.sh <output-jar>
set -eu

out=${1:?usage: build.sh <output-jar>}
here=$(cd "$(dirname "$0")" && pwd)
bin=${JAVA_HOME:+$JAVA_HOME/bin/}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT INT TERM

find "$here/src" -name '*.java' | sort > "$work/sources"
mkdir "$work/classes"
"${bin}javac" --release 8 -Xlint:all,-options -Werror -g:none -d "$work/classes" @"$work/sources"

printf 'Premain-Class: dev.trogon.sandbox.proxyagent.ProxyAgent\nAgent-Class: dev.trogon.sandbox.proxyagent.ProxyAgent\n' > "$work/MANIFEST.MF"
mkdir -p "$(dirname "$out")"
"${bin}jar" --create --file "$out" --manifest "$work/MANIFEST.MF" --no-compress -C "$work/classes" .
echo "built $out"
