"use strict";
// tadmor's only script: the line editor on document and order forms
// (spec/domain.md §13 D2). The Content Security Policy forbids inline
// script. Totals are previewed with exact decimal arithmetic on scaled
// BigInts, rounding half away from zero to 4 places as the database does
// (domain §2), so the preview never disagrees with what is saved.

// A decimal is {n, s}: the value n / 10^s, n a BigInt.
function parseDecimal(text) {
  text = (text || "").trim();
  if (!/^[+-]?(\d+(\.\d*)?|\.\d+)$/.test(text)) return null;
  const negative = text.startsWith("-");
  const [whole, frac = ""] = text.replace(/^[+-]/, "").split(".");
  const n = BigInt((whole || "0") + frac);
  return { n: negative ? -n : n, s: frac.length };
}

// Rounds to `scale` places, half away from zero.
function round(d, scale) {
  if (d.s <= scale) return { n: d.n * 10n ** BigInt(scale - d.s), s: scale };
  const div = 10n ** BigInt(d.s - scale);
  const negative = d.n < 0n;
  const abs = negative ? -d.n : d.n;
  let q = abs / div;
  if ((abs % div) * 2n >= div) q += 1n;
  return { n: negative ? -q : q, s: scale };
}

const mul = (a, b) => ({ n: a.n * b.n, s: a.s + b.s });
const add = (a, b) => {
  const s = Math.max(a.s, b.s);
  return { n: round(a, s).n + round(b, s).n, s };
};
const ZERO = { n: 0n, s: 4 };
const HUNDREDTH = { n: 1n, s: 2 };

// Exact, grouped, at least two places: "1,234.50", "10.0011".
function money(d) {
  const negative = d.n < 0n;
  const digits = (negative ? -d.n : d.n).toString().padStart(d.s + 1, "0");
  const whole = digits.slice(0, digits.length - d.s);
  let frac = digits.slice(digits.length - d.s).replace(/0+$/, "");
  while (frac.length < 2) frac += "0";
  return (negative ? "-" : "") + whole.replace(/\B(?=(\d{3})+(?!\d))/g, ",") + "." + frac;
}

// A line's money (domain §2): inputs rounded to 4 places first, then
//   subtotal = round(qty × price, 4), tax = round(qty × price × rate × 0.01, 4).
function lineMoney(row) {
  const value = (cls) => parseDecimal(row.querySelector(cls).value);
  const qty = value(".qty");
  if (!qty) return null;
  const q = round(qty, 4), p = round(value(".price") || ZERO, 4), r = round(value(".rate") || ZERO, 4);
  const subtotal = round(mul(q, p), 4);
  const tax = round(mul(mul(mul(q, p), r), HUNDREDTH), 4);
  return { subtotal, tax, total: add(subtotal, tax) };
}

function setupLineEditor(form) {
  const data = JSON.parse(document.getElementById("client-data").textContent);
  const tbody = form.querySelector("#lines tbody");
  const template = form.querySelector("#line-template");

  function recompute() {
    let subtotal = ZERO, tax = ZERO, total = ZERO;
    for (const row of tbody.querySelectorAll("tr.line")) {
      const m = lineMoney(row);
      for (const [cls, key] of [[".out-subtotal", "subtotal"], [".out-tax", "tax"], [".out-total", "total"]]) {
        row.querySelector(cls).textContent = m ? money(m[key]) : "";
      }
      if (m) {
        subtotal = add(subtotal, m.subtotal);
        tax = add(tax, m.tax);
        total = add(total, m.total);
      }
    }
    form.querySelector("#sum-subtotal").textContent = money(subtotal);
    form.querySelector("#sum-tax").textContent = money(tax);
    form.querySelector("#sum-total").textContent = money(total);
  }

  function fillTaxRate(row) {
    const code = row.querySelector(".taxcode").value;
    if (code && data.taxes[code] !== undefined) row.querySelector(".rate").value = data.taxes[code];
  }

  tbody.addEventListener("change", (e) => {
    const row = e.target.closest("tr.line");
    if (!row) return;
    if (e.target.classList.contains("product")) {
      const p = data.products[e.target.value];
      if (p) {
        // Description and tax code always; price and account on the sales side.
        row.querySelector(".desc").value = p.description;
        if (p.tax_code) {
          row.querySelector(".taxcode").value = p.tax_code;
          fillTaxRate(row);
        }
        if (p.price !== null) row.querySelector(".price").value = p.price;
        if (p.account !== null) row.querySelector(".account").value = p.account;
      }
    }
    if (e.target.classList.contains("taxcode")) fillTaxRate(row);
    recompute();
  });
  tbody.addEventListener("input", recompute);
  tbody.addEventListener("click", (e) => {
    if (!e.target.classList.contains("remove")) return;
    e.target.closest("tr.line").remove();
    recompute();
  });
  form.querySelector("#add-line").addEventListener("click", () => {
    tbody.appendChild(template.content.cloneNode(true));
    recompute();
  });

  // A new document takes its party's currency, unless one was chosen.
  const party = form.querySelector('select[name="customer_id"], select[name="supplier_id"]');
  const currency = form.querySelector('select[name="currency_code"]');
  let chosen = currency && currency.value !== "";
  if (currency) currency.addEventListener("change", () => (chosen = true));
  if (party && currency) {
    party.addEventListener("change", () => {
      const c = data.partyCurrency[party.value];
      if (c && !chosen) currency.value = c;
    });
  }
  recompute();
}

document.addEventListener("DOMContentLoaded", () => {
  const form = document.getElementById("docform");
  if (form) setupLineEditor(form);
});
