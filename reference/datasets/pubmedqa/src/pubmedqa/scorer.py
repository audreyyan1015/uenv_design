from uenv.sdk import Scorer, ScoreInput, ScoringContext, ScoreResult, read_reference_text
from uenv_reference_rules import score_pubmedqa


class PubmedqaScorer(Scorer):
    """Dataset-specific scoring; reuse rule functions without intermediate bases."""

    def score(self, request: ScoreInput, context: ScoringContext) -> ScoreResult:
        answer, private_data = read_reference_text(request)
        return score_pubmedqa(answer, private_data)
