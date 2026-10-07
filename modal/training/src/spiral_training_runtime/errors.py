class RuntimeRequestError(ValueError):
    """A request cannot be admitted by the compute runtime."""

    def __init__(self, message: str, status_code: int = 400) -> None:
        super().__init__(message)
        self.status_code = status_code
