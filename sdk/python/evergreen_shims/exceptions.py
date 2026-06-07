class EvergreenShimError(Exception):
    """Base exception for EvergreenShims SDK."""
    pass


class APIError(EvergreenShimError):
    """Raised when the API returns a non-success status code."""

    def __init__(self, status_code: int, message: str):
        self.status_code = status_code
        self.message = message
        super().__init__(f"API error {status_code}: {message}")


class ConnectionError(EvergreenShimError):
    """Raised when a connection to the server cannot be established."""
    pass


class TimeoutError(EvergreenShimError):
    """Raised when a request times out."""
    pass
