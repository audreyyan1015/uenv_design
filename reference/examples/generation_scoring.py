"""Example rule: score whether each full generation exactly answers the question.

This illustrates the protocol, not a general reasoning-quality reward model.
"""
from dataclasses import replace
from uenv.sdk import Scorer, ScoreResult, read_reference_text, read_scoring_trajectory


class ExactAnswerProcessScorer(Scorer):
    def score(self, request, context):
        final_answer, private_data = read_reference_text(request)
        expected = private_data["answer"].strip()
        result = ScoreResult.binary(final_answer.strip() == expected)
        generation_rewards = []
        for event in read_scoring_trajectory(request, context):
            if event["kind"] != "generation":
                continue
            generation = event["payload"]
            content = generation["response"]
            if not content or any(part["kind"] != "text" for part in content):
                continue  # This example rule cannot evaluate this generation; do not invent zero.
            answer = "".join(part["text"] for part in content).strip()
            generation_rewards.append({
                "generation_id": generation["generation_id"],
                "reward": float(answer == expected),
            })
        return replace(result, generation_rewards=generation_rewards)
