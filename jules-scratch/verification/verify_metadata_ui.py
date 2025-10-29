
from playwright.sync_api import sync_playwright

def run(playwright):
    browser = playwright.chromium.launch()
    page = browser.new_page()
    page.goto("http://127.0.0.1:8080")

    # Screenshot of edit mode
    page.screenshot(path="jules-scratch/verification/edit_mode.png")

    # Switch to solve mode
    page.get_by_role("button", name="Puzzle").click()
    page.screenshot(path="jules-scratch/verification/solve_mode.png")

    # "Solve" the puzzle (it's blank, so just click a few cells to trigger updates)
    page.locator("canvas").click(position={"x": 50, "y": 50})
    page.locator("canvas").click(position={"x": 100, "y": 100})

    # The puzzle is blank, so it's already "solved".
    # The description should be visible.
    page.screenshot(path="jules-scratch/verification/solved_mode.png")

    browser.close()

with sync_playwright() as playwright:
    run(playwright)
