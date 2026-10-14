"""About 1,000 true-sounding memories about *other* parts of a larger system, none of which bears
on the ledger-api tasks. They model a store that has grown for a year across a monorepo: the
agent must find the few memories that matter among many that don't.

Deliberately avoided topics (they would change a task's right answer): connection pools,
idempotency, serialization/locking, prepared statements, statement timeouts, money and currency
formatting, pagination, rate limiting, JWT signing keys, Postgres versions in CI, integration-test
setup, the balance route, request validation errors, readiness probes, env-var documentation,
log levels. usage: distractors.py OUT.jsonl [N]
"""
import json
import random
import sys

random.seed(20261014)
N = int(sys.argv[2]) if len(sys.argv) > 2 else 1000
COMPONENTS = {
    "web-app": dict(stack="React 18 + Vite", store="Zustand", test="Playwright", deploy="Vercel", team="#web"),
    "mobile-ios": dict(stack="SwiftUI", store="Core Data", test="XCUITest", deploy="TestFlight", team="#mobile"),
    "mobile-android": dict(stack="Jetpack Compose", store="Room", test="Espresso", deploy="Play Console internal track", team="#mobile"),
    "notifications": dict(stack="Go 1.22", store="Redis streams", test="go test", deploy="ArgoCD", team="#messaging"),
    "search": dict(stack="OpenSearch 2.13", store="index aliases", test="pytest", deploy="Terraform", team="#search"),
    "billing-worker": dict(stack="Python 3.12 + Celery", store="Redis", test="pytest", deploy="ArgoCD", team="#billing"),
    "data-warehouse": dict(stack="dbt + Snowflake", store="Snowflake schemas", test="dbt tests", deploy="Airflow", team="#data"),
    "image-service": dict(stack="Rust + axum", store="S3", test="cargo test", deploy="ArgoCD", team="#media"),
    "admin-console": dict(stack="Next.js 14", store="tRPC", test="Cypress", deploy="Vercel", team="#internal-tools"),
    "design-system": dict(stack="Storybook 8 + Radix", store="design tokens JSON", test="Chromatic", deploy="npm publish", team="#design"),
    "infra": dict(stack="Terraform + EKS", store="S3 state bucket", test="terratest", deploy="Atlantis", team="#platform-infra"),
    "docs-site": dict(stack="Docusaurus 3", store="MDX", test="link checker", deploy="GitHub Pages", team="#devrel"),
    "analytics-events": dict(stack="Segment + Kafka", store="Kafka topics", test="schema registry checks", deploy="Confluent Cloud", team="#data"),
}
THINGS = ["the build cache", "the nightly job", "the feature-flag client", "the image resizer", "the push token store",
          "the search reindex", "the email templates", "the CDN purge", "the onboarding flow", "the dark-mode tokens",
          "the crash reporter", "the export to CSV", "the A/B test bucketing", "the sitemap generator", "the font loading",
          "the offline cache", "the deep-link router", "the thumbnail queue", "the webhook relay", "the cost dashboard"]
ALTS = ["Webpack", "Redux", "Jest", "Firebase", "Elasticsearch 7", "RabbitMQ", "BigQuery", "ImageMagick", "Gatsby",
        "styled-components", "CloudFormation", "Mixpanel", "Sentry", "CircleCI", "Lambda", "a cron box", "Puppeteer"]
REASONS = ["it halved cold-start time", "the old one was unmaintained", "it cut the monthly bill by about 40%",
           "it supports offline mode", "the team already knew it", "it removed a flaky dependency",
           "it made screenshots deterministic", "it works behind the corporate proxy", "it has first-class TypeScript types",
           "it let us drop a sidecar"]
SYMPTOMS = ["builds hang at 'optimizing assets'", "thumbnails come out rotated", "the first push after install is dropped",
            "the reindex leaves an orphan alias", "Storybook shows stale tokens", "dbt models time out on Mondays",
            "CSV exports lose leading zeros", "the CDN serves a stale bundle", "dark mode flickers on first paint",
            "deep links open the home screen instead"]
FIXES = ["clear the cache volume and rebuild", "pin the plugin to the previous minor", "re-run the job once",
         "bump the worker memory to 2 GB", "purge the CDN path by hand", "delete the orphan alias with the cleanup script",
         "restart the simulator", "regenerate the tokens with the design script", "set the CSV column type to text",
         "add the missing intent filter"]
DAYS = ["Mondays", "Tuesdays", "Wednesdays", "Thursdays", "Fridays"]
TEMPLATES = [
    ("decision", "{c}: we chose {stack} over {alt} for {thing} because {reason}."),
    ("decision", "{c}: {thing} is owned by {team}; changes need a review from someone in that channel."),
    ("decision", "{c}: releases go out through {deploy} on {day} only, never on Fridays after 3pm."),
    ("assertion", "{c} keeps {thing} in {store}; the key prefix is {slug}:{n}."),
    ("assertion", "{c} runs its {test} suite in about {n} minutes; the slowest part is {thing}."),
    ("assertion", "{c}: the {thing} lives in {path}; nothing else imports it."),
    ("observation", "{c}: {symptom} roughly once a week; the workaround is to {fix}."),
    ("observation", "{c}: {thing} got about {n}% slower after the last {stack} upgrade."),
    ("hypothesis", "{c}: {thing} might move to {alt}-free tooling next quarter; not decided."),
    ("assertion", "{c}: on-call for {thing} is {team}; the runbook is docs/runbooks/{slug}-{n}.md."),
]


def main():
    out, seen = [], set()
    while len(out) < N:
        c = random.choice(list(COMPONENTS))
        meta = COMPONENTS[c]
        claim, template = random.choice(TEMPLATES)
        text = template.format(
            c=c, stack=meta["stack"], store=meta["store"], test=meta["test"], deploy=meta["deploy"], team=meta["team"],
            alt=random.choice(ALTS), thing=random.choice(THINGS), reason=random.choice(REASONS),
            symptom=random.choice(SYMPTOMS), fix=random.choice(FIXES), day=random.choice(DAYS),
            n=random.randint(2, 90), slug=c.replace("-", "_"), path=f"{c}/src/{random.choice(['lib', 'core', 'features', 'shared'])}")
        if text in seen:
            continue
        seen.add(text)
        out.append({"component": c, "claim_type": claim, "layer": "project", "content": text})
    with open(sys.argv[1], "w", encoding="utf-8") as f:
        for row in out:
            f.write(json.dumps(row, ensure_ascii=False) + "\n")
    print(f"{len(out)} memories, {sum(len(r['content']) for r in out):,} chars")


if __name__ == "__main__":
    main()
