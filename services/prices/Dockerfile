FROM python:3.12-slim

# uv resolves the scientific stack (pandas/duckdb/yfinance) reliably.
COPY --from=ghcr.io/astral-sh/uv:latest /uv /usr/local/bin/uv

WORKDIR /app
RUN uv pip install --system --no-cache \
    fastapi uvicorn yfinance duckdb pandas \
    'psycopg[binary]' exchange-calendars

COPY server.py ./
ENV PRICES_DATA=/data
EXPOSE 8001

HEALTHCHECK --interval=10s --timeout=3s --retries=5 \
    CMD python -c "import urllib.request; urllib.request.urlopen('http://localhost:8001/health')" || exit 1

CMD ["uvicorn", "server:app", "--host", "0.0.0.0", "--port", "8001"]
