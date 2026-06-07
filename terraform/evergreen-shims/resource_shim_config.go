package main

import (
	"context"
	"time"

	"github.com/hashicorp/terraform-plugin-sdk/v2/diag"
	"github.com/hashicorp/terraform-plugin-sdk/v2/helper/schema"
)

func resourceShimConfig() *schema.Resource {
	return &schema.Resource{
		CreateContext: resourceShimConfigCreate,
		ReadContext:   resourceShimConfigRead,
		UpdateContext: resourceShimConfigUpdate,
		DeleteContext: resourceShimConfigDelete,
		Importer: &schema.ResourceImporter{
			StateContext: schema.ImportStatePassthroughContext,
		},
		Timeouts: &schema.ResourceTimeout{
			Create: schema.DefaultTimeout(10 * time.Minute),
			Update: schema.DefaultTimeout(10 * time.Minute),
			Delete: schema.DefaultTimeout(10 * time.Minute),
		},
		Schema: map[string]*schema.Schema{
			"name": {
				Type:        schema.TypeString,
				Required:    true,
				ForceNew:    true,
				Description: "Name of the ShimConfig resource.",
			},
			"namespace": {
				Type:        schema.TypeString,
				Optional:    true,
				Default:     "default",
				Description: "Namespace for the ShimConfig resource.",
			},
			"shim_image": {
				Type:        schema.TypeString,
				Required:    true,
				Description: "Container image for the shim.",
			},
			"shim_version": {
				Type:        schema.TypeString,
				Required:    true,
				Description: "Version of the shim to deploy.",
			},
			"target_services": {
				Type:        schema.TypeList,
				Required:    true,
				Elem:        &schema.Schema{Type: schema.TypeString},
				Description: "List of target services to shim.",
			},
			"resource_limits": {
				Type:     schema.TypeMap,
				Optional: true,
				Elem:     &schema.Schema{Type: schema.TypeString},
				Description: "Resource limits for the shim container.",
			},
			"resource_requests": {
				Type:     schema.TypeMap,
				Optional: true,
				Elem:     &schema.Schema{Type: schema.TypeString},
				Description: "Resource requests for the shim container.",
			},
			"env_vars": {
				Type:     schema.TypeMap,
				Optional: true,
				Elem:     &schema.Schema{Type: schema.TypeString},
				Description: "Environment variables for the shim container.",
			},
			"annotations": {
				Type:     schema.TypeMap,
				Optional: true,
				Elem:     &schema.Schema{Type: schema.TypeString},
				Description: "Annotations to apply to the ShimConfig.",
			},
			"labels": {
				Type:     schema.TypeMap,
				Optional: true,
				Elem:     &schema.Schema{Type: schema.TypeString},
				Description: "Labels to apply to the ShimConfig.",
			},
			"created_at": {
				Type:     schema.TypeString,
				Computed: true,
				Description: "Timestamp when the ShimConfig was created.",
			},
		},
	}
}

func resourceShimConfigCreate(ctx context.Context, d *schema.ResourceData, meta interface{}) diag.Diagnostics {
	provider := meta.(*ProviderConfig)
	namespace := d.Get("namespace").(string)
	if namespace == "" {
		namespace = provider.Namespace
	}

	name := d.Get("name").(string)

	d.SetId(name)

	return resourceShimConfigRead(ctx, d, meta)
}

func resourceShimConfigRead(ctx context.Context, d *schema.ResourceData, meta interface{}) diag.Diagnostics {
	provider := meta.(*ProviderConfig)
	_ = provider

	if d.Id() == "" {
		return diag.Diagnostics{}
	}

	return nil
}

func resourceShimConfigUpdate(ctx context.Context, d *schema.ResourceData, meta interface{}) diag.Diagnostics {
	_ = meta

	return resourceShimConfigRead(ctx, d, meta)
}

func resourceShimConfigDelete(ctx context.Context, d *schema.ResourceData, meta interface{}) diag.Diagnostics {
	provider := meta.(*ProviderConfig)
	_ = provider

	d.SetId("")

	return nil
}
