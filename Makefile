.PHONY: install test zip clean

install:
	pip install -e ".[llm,dev]"

test:
	pytest tests/ -v

zip:
	cd .. && zip -r memctrl.zip memctrl/ -x "memctrl/.git/*" "memctrl/__pycache__/*" "memctrl/*.pyc" "memctrl/.pytest_cache/*"

clean:
	rm -rf build/ dist/ *.egg-info .pytest_cache __pycache__
