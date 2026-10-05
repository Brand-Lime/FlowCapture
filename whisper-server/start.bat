@echo off
setlocal

if exist ".\.venv\Scripts\activate.bat" (
	call ".\.venv\Scripts\activate.bat"
) else (
	echo Virtualenv not found. Creating .venv...
	python -m venv .venv
	call ".\.venv\Scripts\activate.bat"
	if exist "requirements.txt" (
		echo Installing requirements...
		pip install -r requirements.txt
	)
)

if exist ".\.venv\Scripts\python.exe" (
	".\.venv\Scripts\python.exe" server.py --host 0.0.0.0 --port 8123 --model large-v3
) else (
	python server.py --host 0.0.0.0 --port 8123 --model large-v3
)
