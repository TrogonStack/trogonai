"""Keep complete coverage evidence while bounding only the published comment."""
from dataclasses import dataclass
from decimal import Decimal
import os
from pathlib import Path
import re
import sys
from urllib.parse import urlparse

MAX_COMMENT_BYTES = 60_000


@dataclass(frozen=True)
class ArtifactLink:
    value: str

    def __post_init__(self):
        parsed = urlparse(self.value)
        if parsed.scheme != 'https' or not parsed.netloc or parsed.username or parsed.password or any(c in self.value for c in '\r\n<>'):
            raise ValueError('Expected the uploaded coverage artifact URL')


def bound_comment():
    complete = Path('.coverage-output.final').read_bytes()
    link = ArtifactLink(os.environ['COVERAGE_ARTIFACT_URL'])
    if len(complete) <= MAX_COMMENT_BYTES:
        comment = complete
    else:
        total = Decimal(Path('.coverage-total').read_text().strip())
        threshold = Decimal(os.environ['COVERAGE_THRESHOLD'])
        if not total.is_finite() or not 0 <= total <= 100 or not threshold.is_finite():
            raise ValueError('Invalid coverage summary percentage')
        commit = os.environ.get('COVERAGE_HEAD_SHA') or os.environ['GITHUB_SHA']
        if not re.fullmatch('[0-9a-f]{40}', commit):
            raise ValueError('Expected immutable coverage source identity')
        comment = (
            '# Code coverage report\n\n'
            f'Measured coverage: {total}%. Minimum required coverage: {threshold}%.\n\n'
            'The complete report exceeds the comment size limit. '
            f'[Download the full report and XML evidence](<{link.value}>).\n\n'
            'Coverage thresholds and comparison checks are evaluated separately.\n\n'
            f'Results for commit: {commit}\n'
        ).encode()
    if len(comment) > MAX_COMMENT_BYTES or len(comment.decode().encode('utf-16-le')) // 2 > MAX_COMMENT_BYTES:
        raise ValueError('Coverage comment exceeds its bounded publication contract')
    Path('.coverage-output.comment').write_bytes(comment)


if __name__ == '__main__':
    bound_comment()
