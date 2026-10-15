// The book lives under /docs/; the title in the menu bar leads back to the
// landing page at the site root.
(function () {
  var title = document.querySelector(".menu-title");
  if (!title || typeof path_to_root === "undefined") return;
  var link = document.createElement("a");
  link.href = path_to_root + "../";
  link.textContent = title.textContent;
  link.style.color = "inherit";
  link.style.textDecoration = "none";
  title.textContent = "";
  title.appendChild(link);
})();
