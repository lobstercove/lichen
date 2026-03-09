from setuptools import setup, find_packages

with open("README.md", "r", encoding="utf-8") as fh:
    long_description = fh.read()

setup(
    name="moltchain-sdk",
    version="0.1.0",
    author="Trading Lobster",
    author_email="trading.lobster@moltchain.io",
    description="Official Python SDK for MoltChain blockchain",
    long_description=long_description,
    long_description_content_type="text/markdown",
    url="https://github.com/lobstercove/moltchain",
    project_urls={
        "Bug Tracker": "https://github.com/lobstercove/moltchain/issues",
        "Documentation": "https://developers.moltchain.network",
    },
    classifiers=[
        "Development Status :: 4 - Beta",
        "Intended Audience :: Developers",
        "Topic :: Software Development :: Libraries :: Python Modules",
        "License :: OSI Approved :: MIT License",
        "Programming Language :: Python :: 3",
        "Programming Language :: Python :: 3.8",
        "Programming Language :: Python :: 3.9",
        "Programming Language :: Python :: 3.10",
        "Programming Language :: Python :: 3.11",
        "Programming Language :: Python :: 3.12",
    ],
    packages=find_packages(),
    python_requires=">=3.8",
    install_requires=[
        "httpx>=0.25.0",
        "websockets>=12.0",
        "base58>=2.1.1",
        "pynacl>=1.5.0",
    ],
    extras_require={
        "dev": [
            "pytest>=7.4.0",
            "pytest-asyncio>=0.21.0",
            "black>=23.0.0",
            "mypy>=1.5.0",
        ],
    },
)
