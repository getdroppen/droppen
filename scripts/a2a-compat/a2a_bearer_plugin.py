"""A pytest plugin for the A2A TCK: `Authorization: Bearer $A2A_BEARER_TOKEN` on
every httpx client the TCK opens.

The TCK has no option for the credentials of the agent it tests, and
Droppen's `/a2a/*` answers nothing without an A2A token. Loaded by
scripts/a2a-compat/tck.sh: PYTHONPATH=<this directory> run_tck.py ... -- -p a2a_bearer_plugin
"""

import os

import httpx

_TOKEN = os.environ.get("A2A_BEARER_TOKEN", "")


def _with_bearer(cls: type) -> None:
    original = cls.__init__

    def __init__(self, *args, **kwargs):  # noqa: N807
        if _TOKEN:
            headers = httpx.Headers(kwargs.get("headers") or {})
            headers.setdefault("Authorization", f"Bearer {_TOKEN}")
            kwargs["headers"] = headers
        original(self, *args, **kwargs)

    cls.__init__ = __init__


_with_bearer(httpx.Client)
_with_bearer(httpx.AsyncClient)
