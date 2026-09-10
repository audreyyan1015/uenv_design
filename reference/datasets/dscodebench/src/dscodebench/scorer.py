from uenv.sdk import Scorer, ScoreInput, ScoringContext, ScoreResult, evaluate_harness


class DscodebenchScorer(Scorer):
    """Dataset-specific scoring using the Worker-bound harness function."""

    def score(self, request: ScoreInput, context: ScoringContext) -> ScoreResult:
        return evaluate_harness(request, context)
