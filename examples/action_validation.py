"""Safe action validation entry point: state query, optional StandUp/StandDown.

The previous implicit mode/VUI/controller and multi-owner commands are deliberately
not part of this example. Use the raw SDK only with explicit device-specific approval.
"""

from example_support import run
from sport_mode import main

if __name__ == "__main__":
    run(main)
