const copyButton = document.querySelector("[data-copy]");

if (copyButton) {
  copyButton.addEventListener("click", async () => {
    const text = copyButton.getAttribute("data-copy");
    if (!text) return;

    try {
      await navigator.clipboard.writeText(text);
      const previous = copyButton.textContent;
      copyButton.textContent = "Copied";
      copyButton.classList.add("is-copied");
      setTimeout(() => {
        copyButton.textContent = previous;
        copyButton.classList.remove("is-copied");
      }, 1400);
    } catch (_) {
      // Clipboard can fail on some browsers/pages; keep the page usable.
    }
  });
}

const observer = new IntersectionObserver(
  (entries) => {
    entries.forEach((entry) => {
      if (entry.isIntersecting) {
        entry.target.classList.add("is-visible");
        observer.unobserve(entry.target);
      }
    });
  },
  { threshold: 0.1 }
);

document.querySelectorAll(".reveal").forEach((node) => observer.observe(node));

const lightbox = document.getElementById("screenshot-lightbox");
const lightboxImage = document.getElementById("lightbox-image");
const lightboxCaption = document.getElementById("lightbox-caption");
const lightboxCounter = document.getElementById("lightbox-counter");
const lightboxClose = document.querySelector("[data-lightbox-close]");
const lightboxPrev = document.querySelector("[data-lightbox-prev]");
const lightboxNext = document.querySelector("[data-lightbox-next]");
const screenshotButtons = Array.from(document.querySelectorAll("[data-lightbox-item]"));

if (
  lightbox &&
  lightboxImage &&
  lightboxCaption &&
  lightboxCounter &&
  lightboxClose &&
  lightboxPrev &&
  lightboxNext &&
  screenshotButtons.length > 0
) {
  const screenshots = screenshotButtons.map((button) => ({
    full: button.getAttribute("data-full") || "",
    title: button.getAttribute("data-title") || "",
    caption: button.getAttribute("data-caption") || "",
    alt: button.querySelector("img")?.getAttribute("alt") || ""
  }));

  let currentIndex = 0;
  let lastFocusedButton = null;

  const renderLightbox = () => {
    const shot = screenshots[currentIndex];
    lightboxImage.src = shot.full;
    lightboxImage.alt = shot.alt;
    lightboxCaption.textContent = `${shot.title} - ${shot.caption}`;
    lightboxCounter.textContent = `${currentIndex + 1} / ${screenshots.length}`;
  };

  const openLightbox = (index, sourceButton) => {
    currentIndex = (index + screenshots.length) % screenshots.length;
    lastFocusedButton = sourceButton;
    renderLightbox();
    lightbox.classList.remove("hidden");
    lightbox.classList.add("flex");
    lightbox.setAttribute("aria-hidden", "false");
    document.body.style.overflow = "hidden";
    lightboxClose.focus();
  };

  const closeLightbox = () => {
    lightbox.classList.add("hidden");
    lightbox.classList.remove("flex");
    lightbox.setAttribute("aria-hidden", "true");
    document.body.style.overflow = "";
    lightboxImage.src = "";
    if (lastFocusedButton instanceof HTMLElement) {
      lastFocusedButton.focus();
    }
  };

  const stepLightbox = (delta) => {
    currentIndex = (currentIndex + delta + screenshots.length) % screenshots.length;
    renderLightbox();
  };

  screenshotButtons.forEach((button, index) => {
    button.addEventListener("click", () => openLightbox(index, button));
  });

  lightboxClose.addEventListener("click", closeLightbox);
  lightboxPrev.addEventListener("click", () => stepLightbox(-1));
  lightboxNext.addEventListener("click", () => stepLightbox(1));

  lightbox.addEventListener("click", (event) => {
    if (event.target === lightbox) {
      closeLightbox();
    }
  });

  document.addEventListener("keydown", (event) => {
    if (lightbox.classList.contains("hidden")) return;

    if (event.key === "Escape") {
      closeLightbox();
    }
    if (event.key === "ArrowLeft") {
      stepLightbox(-1);
    }
    if (event.key === "ArrowRight") {
      stepLightbox(1);
    }
  });
}

function formatStars(value) {
  if (value < 1000) return String(value);
  const rounded = Math.round((value / 1000) * 10) / 10;
  return `${rounded}k`;
}

const starsTarget = document.getElementById("github-stars");

if (starsTarget) {
  fetch("https://api.github.com/repos/penso/arbor")
    .then((response) => {
      if (!response.ok) throw new Error("request_failed");
      return response.json();
    })
    .then((data) => {
      if (typeof data.stargazers_count === "number") {
        starsTarget.textContent = formatStars(data.stargazers_count);
      } else {
        starsTarget.textContent = "Star";
      }
    })
    .catch(() => {
      starsTarget.textContent = "Star";
    });
}
