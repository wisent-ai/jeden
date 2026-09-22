"""What the schema has to say, and whether the golden envelopes obey it."""

from .checks import _check_schema_contract
from .golden import _check_golden

__all__ = ["_check_schema_contract", "_check_golden"]
