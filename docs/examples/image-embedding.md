Image Embedding Test
===============================================================================

Open this file in mudl to test embedded image syntax. Each section exercises
a different piece of Markdown image syntax — alt text, a title attribute,
relative-path resolution, an image inside a link — rather than showing off any
particular picture; every image below points at the same 1x1 placeholder.


## Basic images

A relative path to an image in the `Resources/` subdirectory.

![First placeholder image](Resources/placeholder.png)

![Second placeholder image](Resources/placeholder.png)


## Parent directory path

This exercises a `../` relative path: up one directory from this file and
back down into `Resources/`.

![Placeholder image via a parent-relative path](../examples/Resources/placeholder.png)


## Image with title attribute

Hover over this image to see the title tooltip.

![Placeholder image](Resources/placeholder.png "This is the title attribute")


## Inline images

Images placed inline with surrounding text.

Here is an icon ![placeholder](Resources/placeholder.png) embedded in a
paragraph of text. The image should appear inline with the words around it.


## Multiple images in sequence

Two images back-to-back with no text between them.

![First image](Resources/placeholder.png)

![Second image](Resources/placeholder.png)


## Image inside a link

Clicking this image should behave like a regular external link.

[![Placeholder image](Resources/placeholder.png)](https://example.com)


## Image with empty alt text

Decorative image with empty alt text.

![](Resources/placeholder.png)


## Remote image

This image is loaded from the web over HTTPS.

![Avatar](https://josephpearson.org/avatars/floating.png)


## Broken image reference

This references a file that does not exist. It should display alt text or a
broken-image indicator.

![This image does not exist](nonexistent.png)


## Expected behavior summary

| Scenario          | Expected result                                   |
| ----------------- | -------------------------------------------------- |
| Basic image       | Displays the image from a relative path           |
| Parent directory  | Resolves `../` and displays the image             |
| Title attribute   | Shows tooltip on hover                            |
| Inline image      | Image appears inline within the paragraph         |
| Sequential images | Both images display stacked vertically            |
| Linked image      | Image is clickable; opens link in default browser |
| Empty alt text    | Image displays with no alt text                   |
| Remote image      | Loads and displays image from the web             |
| Broken reference  | Shows alt text or broken-image placeholder        |
