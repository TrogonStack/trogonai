# shellcheck shell=bash
# Sourced by the github:workflows:*:result tasks. Branch rules require one
# result check per workflow because job names like lint and build repeat across
# workflows, and a skipped job passes a required check even when a cancelled
# run skipped it.

# ci_result LABEL
#
# Reads SELECTION_RESULT (the selection job result), SELECTED (whether
# selection picked this workflow's suite) and JOB_RESULTS (space separated job
# results). A cancelled run also skips jobs, so a skip only passes when
# selection succeeded and found nothing to check.
ci_result() {
  local label=$1
  local selection_result=${SELECTION_RESULT:?SELECTION_RESULT is required}
  local selected=${SELECTED:-}
  local job_results=${JOB_RESULTS:?JOB_RESULTS is required}
  local results result

  read -r -a results <<<"$job_results"
  for result in "${results[@]}"; do
    if [[ "$result" == success ]]; then continue; fi
    if [[ "$result" == skipped && "$selection_result" == success && "$selected" != true ]]; then continue; fi
    echo "::error::$label ended with $job_results after selection ended with $selection_result"
    return 1
  done
}
