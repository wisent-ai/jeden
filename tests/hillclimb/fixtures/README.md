# Greeter qualification target

This is an exclusively reserved, reusable Jeden hillclimb qualification repository,
not an operator project. Its baseline tracks only this README and `greet.mjs`.
`node greet.mjs NAME` prints `Hello, NAME`.

The real-product journey adds missing CLI capabilities and verifies their actual
behavior and independent acceptance. It retains the observed changes and receipts,
then restores the clean baseline for the next journey. Generated untracked and
ignored files are removed during that restoration.

Do not commit, push, change git metadata or create remotes. The journey refuses to
rewind a changed HEAD, branch or origin. This fixture has one canonical checkout
on `main`; it is never cloned or initialized inside another checkout.
