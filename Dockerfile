FROM python:3.12-slim

WORKDIR /app

# Install uv for fast dependency management
COPY --from=ghcr.io/astral-sh/uv:latest /uv /uvx /bin/

# Copy dependency files first for better layer caching
COPY pyproject.toml uv.lock ./

# Install dependencies
RUN uv sync --frozen --no-dev --no-install-project

# Copy application files
COPY check_hallucinated_references.py .
COPY app.py .
COPY templates/ templates/
COPY static/ static/

EXPOSE 5001

CMD ["uv", "run", "python", "app.py"]
