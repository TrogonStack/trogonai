# shellcheck shell=bash
# Sourced by the github:workflows:ci-bazel:* Bazel tasks.
#
# On a self-hosted runner with a Bazel remote cache, the remote-cache task
# writes $RUNNER_TEMP/bazel-remote.bazelrc and the cache lives on that server,
# so the GitHub Actions disk and repository caches are not used. Everywhere
# else the tasks use those caches exactly as before.

# bazel_ci COMMAND [ARG...]
#
# Runs `bazel COMMAND` with the cache flags that fit the runner.
bazel_ci() {
  local command=$1
  shift
  local rc="${RUNNER_TEMP:?}/bazel-remote.bazelrc"
  local startup=() cache=()

  if [[ -f $rc ]]; then
    startup=("--bazelrc=$rc")
  else
    cache=("--disk_cache=$RUNNER_TEMP/bazel-disk-cache" "--repository_cache=$RUNNER_TEMP/bazel-repository-cache")
  fi

  exec bazel ${startup[@]+"${startup[@]}"} "$command" "$@" ${cache[@]+"${cache[@]}"}
}
