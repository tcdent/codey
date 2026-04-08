"""Pytest fixtures for integration tests."""

import pytest
from harness import CodeySession, LLMEvaluator


@pytest.fixture(scope="session")
def evaluator():
    """Shared LLM evaluator instance."""
    return LLMEvaluator()


@pytest.fixture
def codey():
    """
    A fresh codey session for each test.
    Starts before the test, killed after.
    """
    session = CodeySession()
    session.start()
    yield session
    session.stop()
