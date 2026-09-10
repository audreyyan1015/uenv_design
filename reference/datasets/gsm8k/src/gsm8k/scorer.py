from uenv.sdk import Scorer, ScoreInput, ScoringContext, ScoreResult, read_reference_text
from uenv_reference_rules import score_gsm8k


class Gsm8kScorer(Scorer):
    """Dataset-specific scoring; reuse rule functions without intermediate bases."""

    def score(self, request: ScoreInput, context: ScoringContext) -> ScoreResult:
        answer, private_data = read_reference_text(request)
        return score_gsm8k(answer, private_data)
