// Load Mermaid from CDN and initialize on every page.
// Uses the 'base' theme with light node backgrounds and darker text,
// plus lightened line/arrow colors for readability on dark page themes.
(function() {
  var script = document.createElement('script');
  script.src = 'https://cdn.jsdelivr.net/npm/mermaid@11/dist/mermaid.min.js';
  script.onload = function() {
    mermaid.initialize({
      startOnLoad: true,
      theme: 'base',
      themeVariables: {
        // Node backgrounds — light, readable on dark navy
        primaryColor: '#334455',
        primaryBorderColor: '#556677',
        primaryTextColor: '#e0e0e0',
        secondaryColor: '#3a4d5e',
        secondaryBorderColor: '#556677',
        secondaryTextColor: '#e0e0e0',
        tertiaryColor: '#405568',
        tertiaryBorderColor: '#667788',
        tertiaryTextColor: '#e0e0e0',
        // Note boxes
        noteBkgColor: '#3a4d5e',
        noteBorderColor: '#556677',
        noteTextColor: '#e0e0e0',
        // Lines and arrows
        lineColor: '#8899aa',
        // Sequence diagram actors
        actorBkg: '#334455',
        actorBorder: '#556677',
        actorTextColor: '#e0e0e0',
        actorLineColor: '#778899',
        // Sequence diagram signals
        signalColor: '#8899aa',
        signalTextColor: '#e0e0e0',
        // Sequence diagram labels
        labelBoxBkgColor: '#334455',
        labelBoxBorderColor: '#556677',
        labelTextColor: '#e0e0e0',
        loopTextColor: '#e0e0e0',
      }
    });
  };
  document.head.appendChild(script);
})();
