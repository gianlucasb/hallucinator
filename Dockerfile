FROM python:3.11-slim

WORKDIR /app

# Install dependencies first for better layer caching
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

# Copy application files
COPY check_hallucinated_references.py .
COPY app.py .
COPY templates/ templates/
COPY static/ static/

EXPOSE 5001

CMD ["python", "app.py"]
